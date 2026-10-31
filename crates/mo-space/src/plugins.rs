//! The plugins installed on this computer, from the plugin files in the plugin folders: the C# app's
//! `PluginCatalog.ScanFiles`, `DefaultFolders`, `Family` and `IsWaves`, as Space needs them.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use mo_formats::case;
use mo_formats::plugins::{format_from_path, name_from_path};
use mo_formats::textscan::is_plugin_path;
use regex::Regex;

use crate::key;

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
    // .NET's char.IsLetterOrDigit: letters and decimal digits.
    case::to_lower(&n).chars().filter(|c| c.is_alphabetic() || c.is_numeric()).collect()
}

/// Waves plugins all live inside one WaveShell file whose version changes with every update.
pub fn is_waves(name: &str, path: &str) -> bool {
    let starts = |s: &str, start: &str| s.get(..start.len()).is_some_and(|p| case::eq_ignore_case(p, start));
    starts(name, "Waves ") || starts(name, "WaveShell") || case::key(path).contains("WAVESHELL")
}

/// The standard plugin folders for this operating system.
pub fn default_folders() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
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
/// link in their place are followed (their plugins are still installed), each real folder once. Folders that can't be
/// read are left out.
pub fn scan_files(folders: &[PathBuf]) -> Vec<InstalledPlugin> {
    let mut found = Vec::new();
    let mut seen_files: HashSet<String> = HashSet::new();
    let mut seen_dirs: HashSet<String> = HashSet::new();
    for folder in folders {
        let mut level = vec![folder.clone()];
        for _ in 0..DEEPEST {
            let mut next = Vec::new();
            for dir in level {
                let real =
                    fs::canonicalize(&dir).map_or_else(|_| key(&dir.to_string_lossy()), |r| key(&r.to_string_lossy()));
                if !seen_dirs.insert(real) {
                    continue;
                }
                let Ok(entries) = fs::read_dir(&dir) else { continue };
                for entry in entries.flatten() {
                    let path = entry.path();
                    let text = path.to_string_lossy().into_owned();
                    // The entry itself, or where its link leads.
                    let is_dir = fs::metadata(&path).is_ok_and(|m| m.is_dir());
                    if is_plugin_path(&text) {
                        let name = name_from_path(&text);
                        // Two letters is a real name: KORG's M1 is M1.vst3 and M1.dll.
                        if name.chars().count() >= 2 && seen_files.insert(key(&text)) {
                            found.push(InstalledPlugin {
                                name,
                                format: format_from_path(&text).to_owned(),
                                path: text.clone(),
                            });
                        }
                        // A bundle's insides aren't plugins of their own.
                        if BUNDLES.iter().any(|b| case::eq_ignore_case(mo_formats::paths::extension(&text), b)) {
                            continue;
                        }
                    }
                    if is_dir {
                        next.push(path);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            level = next;
        }
    }
    found
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
