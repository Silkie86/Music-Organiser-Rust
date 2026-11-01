//! How much room each installed plugin takes: the C# app's `PluginSpace`.
//!
//! Windows doesn't record which folders belong to which plugin, so besides the plugin files themselves this looks for
//! folders named after the plugin or its maker in the usual places (Documents, AppData, ProgramData, Program Files):
//! "Serum Presets", "UVI\UVI Workstation", "Toontrack". Folders you add or rule out for a plugin win over the guess.
//! A folder two plugins claim is counted for neither alone, but as shared. Nothing here removes anything.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_formats::case;
use mo_formats::paths::file_name;
use mo_library::read::name_key;
use mo_library::scan::is_inside;

use crate::plugins::{InstalledPlugin, family, norm};
use crate::size::{self, Measured, Stopped};
use crate::{key, real_path};

/// What a plugin's room on disk is: the plugin itself, its presets and settings, or its sounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SpaceKind {
    Program,
    Presets,
    Sounds,
}

/// One of a plugin's files or folders and how much it takes. `shared_with` names the other plugins (by name key) whose
/// folder it also is (a maker's folder, or Kontakt 7 and Kontakt 8's), so it's never counted twice. `added` is a
/// folder you added yourself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceFolder {
    pub path: String,
    pub kind: SpaceKind,
    pub bytes: u64,
    pub shared_with: Vec<String>,
    pub added: bool,
}

impl SpaceFolder {
    pub fn shared(&self) -> bool {
        !self.shared_with.is_empty()
    }
}

/// A plugin's room on disk: its files and folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginSpace {
    /// The name key, as the Plugins page groups plugins.
    pub key: String,
    pub name: String,
    pub vendor: String,
    pub folders: Vec<SpaceFolder>,
}

impl PluginSpace {
    fn sum(&self, kind: SpaceKind) -> u64 {
        self.folders.iter().filter(|f| !f.shared() && f.kind == kind).map(|f| f.bytes).sum()
    }
    pub fn program(&self) -> u64 {
        self.sum(SpaceKind::Program)
    }
    pub fn presets(&self) -> u64 {
        self.sum(SpaceKind::Presets)
    }
    pub fn sounds(&self) -> u64 {
        self.sum(SpaceKind::Sounds)
    }
    /// Its own room: the folders it shares with other plugins left out.
    pub fn bytes(&self) -> u64 {
        self.program() + self.presets() + self.sounds()
    }
    pub fn shared_bytes(&self) -> u64 {
        self.folders.iter().filter(|f| f.shared()).map(|f| f.bytes).sum()
    }
}

/// An installed plugin: its name key, name, maker and plugin files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub key: String,
    pub name: String,
    pub vendor: String,
    pub files: Vec<String>,
}

/// Where plugins keep presets, settings and sound libraries on this operating system (the ones that are there).
pub fn data_roots() -> Vec<PathBuf> {
    let home = crate::home_folder();
    let roots: Vec<PathBuf> = if cfg!(windows) {
        windows_data_roots()
    } else if cfg!(target_os = "macos") {
        let mut r = vec![PathBuf::from("/Library/Application Support"), PathBuf::from("/Library/Audio/Presets")];
        if let Some(h) = home {
            r.extend(["Library/Application Support", "Documents", "Music", "Library/Audio/Presets"].map(|p| h.join(p)));
        }
        r
    } else {
        home.map(|h| [".local/share", ".config", "Documents"].map(|p| h.join(p)).to_vec()).unwrap_or_default()
    };
    let mut seen = HashSet::new();
    roots.into_iter().filter(|r| r.is_dir() && seen.insert(key(&r.to_string_lossy()))).collect()
}

/// Documents (wherever you've moved it), Public Documents, AppData, ProgramData and the program folders, as Windows
/// says they are.
#[cfg(windows)]
fn windows_data_roots() -> Vec<PathBuf> {
    use known_folders::{KnownFolder, get_known_folder_path};
    [
        KnownFolder::Documents,
        KnownFolder::PublicDocuments,
        KnownFolder::RoamingAppData,
        KnownFolder::LocalAppData,
        KnownFolder::ProgramData,
        KnownFolder::ProgramFiles,
        KnownFolder::ProgramFilesX86,
        KnownFolder::ProgramFilesCommon,
        KnownFolder::ProgramFilesCommonX86,
    ]
    .into_iter()
    .filter_map(get_known_folder_path)
    .collect()
}

#[cfg(not(windows))]
fn windows_data_roots() -> Vec<PathBuf> {
    Vec::new()
}

/// Whether a folder is where programs are installed: those go with Uninstall, never Clean up.
pub fn is_install_folder(path: &str) -> bool {
    cfg!(windows)
        && ["ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .any(|pf| !pf.is_empty() && is_inside(path, &pf))
}

/// Folders that hold a DAW, the system or this app rather than a plugin's things. Never matched, even when a plugin
/// shares the name (PreSonus makes plugins and Studio One).
const NEVER_NAMES: [&str; 44] = [
    "imageline",
    "flstudio",
    "bitwigstudio",
    "bitwig",
    "studioone",
    "presonus",
    "fenderstudio",
    "ableton",
    "cakewalk",
    "reaper",
    "steinberg",
    "cubase",
    "microsoft",
    "windows",
    "windowsapps",
    "packages",
    "temp",
    "programs",
    "commonfiles",
    "musicorganiser",
    "google",
    "mozilla",
    "apple",
    "nvidia",
    "intel",
    "amd",
    "realtek",
    "adobe",
    "claude",
    "anthropic",
    "vst",
    "vst2",
    "vst3",
    "vstplugins",
    "clap",
    "plugins",
    "audio",
    "music",
    "samples",
    "projects",
    "presets",
    "data",
    "content",
    "library",
];

/// Endings a folder named after a plugin may have: "Serum Presets", "Omnisphere Library", "Kontakt 8".
const ENDINGS: [&str; 20] = [
    "",
    "presets",
    "preset",
    "userpresets",
    "data",
    "library",
    "libraries",
    "content",
    "factory",
    "factorycontent",
    "soundbanks",
    "soundbank",
    "sounds",
    "samples",
    "expansions",
    "banks",
    "settings",
    "x64",
    "vst",
    "vst3",
];

/// Whether a folder's name is a plugin's: the plugin's family (its name without a version), perhaps with a version
/// number and one of the usual endings. Names under four letters must match exactly ("M1" never takes "M1 Backups").
pub fn name_matches(folder_name: &str, family: &str) -> bool {
    let name = norm(folder_name);
    let letters = family.chars().count();
    if letters < 2 || name.is_empty() {
        return false;
    }
    if letters < 4 {
        return name == family;
    }
    let Some(rest) = name.strip_prefix(family) else { return false };
    ENDINGS.contains(&rest.trim_start_matches(|c: char| c.is_ascii_digit()))
}

/// Whether a folder is the maker's: its name is the maker's name ("UVI", "Toontrack"), or the start of it ("Xfer" for
/// Xfer Records, "Arturia" for "Arturia SA").
fn maker_matches(folder_name: &str, maker: &str) -> bool {
    let name = norm(folder_name);
    if maker.chars().count() < 3 || name.chars().count() < 3 {
        return false;
    }
    if name == maker {
        return true;
    }
    name.chars().count() >= 4
        && maker.strip_prefix(name.as_str()).is_some_and(|rest| rest.chars().next().is_some_and(|c| !c.is_numeric()))
}

/// A guessed folder. A maker's folder holds plugin folders counted on their own: [`measure`] counts it without them.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Guessed {
    path: String,
}

/// Each plugin's guessed data folders. A folder in a data root named after a plugin is that plugin's. A folder named
/// after a maker is looked into: its folders named after one of the maker's plugins are that plugin's, and the rest of
/// it belongs to all the maker's plugins together.
fn guess(
    plugins: &[Installed],
    data_roots: &[PathBuf],
    plugin_roots: &[String],
    protected_roots: &[String],
) -> HashMap<String, Vec<Guessed>> {
    let mut result: HashMap<String, Vec<Guessed>> = HashMap::new();
    let families: Vec<Fam<'_>> = plugins
        .iter()
        .map(|p| {
            let family = family(&p.name);
            Fam { key: &p.key, version: version_after(&norm(&p.name), &family), family }
        })
        .filter(|f| f.family.chars().count() >= 2)
        .collect();
    // Makers in the order they first come, each with its plugins.
    let mut makers: Vec<(String, Vec<&str>)> = Vec::new();
    for p in plugins.iter().filter(|p| !p.vendor.is_empty()) {
        let maker = norm(&p.vendor);
        if maker.chars().count() < 3 {
            continue;
        }
        match makers.iter_mut().find(|(m, _)| *m == maker) {
            Some((_, keys)) => keys.push(&p.key),
            None => makers.push((maker, vec![&p.key])),
        }
    }
    let off = |dir: &str| {
        NEVER_NAMES.contains(&norm(file_name(dir)).as_str())
            || plugin_roots.iter().chain(protected_roots).any(|r| is_inside(r, dir) || is_inside(dir, r))
    };
    // The longest name wins. Of plugins with the same name but for their version (Kontakt 7 and Kontakt 8), a folder
    // naming a version ("Kontakt 8 Library", "Serum 2 Presets") is that version's; one naming none is theirs together.
    let plugins_named = |dir: &str, among: &[Fam<'_>]| -> Vec<String> {
        let name = file_name(dir);
        let hits: Vec<&Fam<'_>> = among.iter().filter(|f| name_matches(name, &f.family)).collect();
        let longest = hits.iter().map(|f| f.family.chars().count()).max().unwrap_or(0);
        let hits: Vec<&Fam<'_>> = hits.into_iter().filter(|f| f.family.chars().count() == longest).collect();
        let named_version = hits.first().map(|f| version_after(&norm(name), &f.family)).unwrap_or_default();
        let that_version: Vec<&Fam<'_>> =
            hits.iter().copied().filter(|f| !named_version.is_empty() && f.version == named_version).collect();
        let chosen = if that_version.is_empty() { hits } else { that_version };
        chosen.iter().map(|f| f.key.to_owned()).collect()
    };
    let mut add = |key: String, g: Guessed| result.entry(key).or_default().push(g);
    for root in data_roots {
        for dir in sub_folders(root) {
            if off(&dir) {
                continue;
            }
            let named = plugins_named(&dir, &families);
            if !named.is_empty() {
                for k in named {
                    add(k, Guessed { path: dir.clone() });
                }
                continue;
            }
            let Some((_, keys)) = makers.iter().find(|(m, _)| maker_matches(file_name(&dir), m)) else { continue };
            let theirs: Vec<Fam<'_>> = families.iter().filter(|f| keys.contains(&f.key)).cloned().collect();
            for sub in sub_folders(Path::new(&dir)) {
                if off(&sub) {
                    continue;
                }
                for k in plugins_named(&sub, &theirs) {
                    add(k, Guessed { path: sub.clone() });
                }
            }
            for k in keys {
                add((*k).to_owned(), Guessed { path: dir.clone() });
            }
        }
    }
    result
}

/// A plugin's family, with the version its name gives ("8" for Kontakt 8, "" for Serum).
#[derive(Clone)]
struct Fam<'a> {
    key: &'a str,
    family: String,
    version: String,
}

/// The number right after the family in a name made by [`norm`]: "8" in "kontakt8library" after "kontakt".
fn version_after(name: &str, family: &str) -> String {
    name.strip_prefix(family).map(|rest| rest.chars().take_while(char::is_ascii_digit).collect()).unwrap_or_default()
}

/// The folders in a folder, by name; links to other places left out (a folder moved to another drive isn't guessed
/// again).
fn sub_folders(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()))
        .map(|e| e.path().to_string_lossy().into_owned())
        .collect();
    out.sort_by_cached_key(|p| case::key(p));
    out
}

fn kind_of(m: Measured) -> SpaceKind {
    if m.bytes > 0 && m.sound_bytes.saturating_mul(2) >= m.bytes { SpaceKind::Sounds } else { SpaceKind::Presets }
}

/// What a plugin's folders are, beyond the guess: the ones you added and the ones you said aren't its.
pub struct Choices<'a> {
    pub added: &'a dyn Fn(&str) -> Vec<String>,
    pub ruled_out: &'a dyn Fn(&str) -> Vec<String>,
}

/// Measures the installed plugins. `plugin_roots` are the plugin folders (never data, so the plugin files aren't
/// counted twice); `protected_roots` are your project and sample folders, never counted as a plugin's. Every plugin's
/// folders are guessed, so the shared ones are known, but only `only`'s are measured when it's given.
pub fn measure(
    plugins: &[Installed],
    data_roots: &[PathBuf],
    plugin_roots: &[String],
    protected_roots: &[String],
    choices: &Choices<'_>,
    only: Option<&HashSet<String>>,
    stop: &AtomicBool,
) -> Result<Vec<PluginSpace>, Stopped> {
    let guesses = guess(plugins, data_roots, plugin_roots, protected_roots);
    let mut sizes: HashMap<String, Measured> = HashMap::new();
    let mut size_of = |path: &str| -> Result<Measured, Stopped> {
        if let Some(m) = sizes.get(&key(path)) {
            return Ok(*m);
        }
        // A plugin file or bundle that's a link (moved to another drive) is measured where it really is.
        let real = if size::is_link(Path::new(path)) { real_path(path) } else { None };
        let m = size::measure(Path::new(real.as_deref().unwrap_or(path)), stop)?;
        sizes.insert(key(path), m);
        Ok(m)
    };

    // Folders you added are the plugin's alone: no other plugin's guess takes them. The ones you ruled out are left out
    // of every guess for it.
    let added: HashMap<&str, Vec<String>> = plugins
        .iter()
        .map(|p| (p.key.as_str(), (choices.added)(&p.key).into_iter().filter(|a| Path::new(a).is_dir()).collect()))
        .collect();
    // Who added each folder: two plugins can both have it (Kontakt 7 and Kontakt 8 using one library), and then it's
    // shared, as a guessed folder two plugins claim is.
    let mut added_by: HashMap<String, Vec<&str>> = HashMap::new();
    for (k, folders) in &added {
        for f in folders {
            let list = added_by.entry(key(f)).or_default();
            if !list.contains(k) {
                list.push(k);
            }
        }
    }
    let mut claimed: HashMap<&str, (Vec<Guessed>, Vec<String>)> = HashMap::new();
    for p in plugins {
        let no: HashSet<String> = (choices.ruled_out)(&p.key).iter().map(|f| key(f)).collect();
        let guessed: Vec<Guessed> = guesses
            .get(&p.key)
            .map(|g| {
                g.iter()
                    .filter(|f| !no.contains(&key(&f.path)))
                    .filter(|f| added_by.get(&key(&f.path)).is_none_or(|ks| ks.contains(&p.key.as_str())))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        claimed.insert(&p.key, (guessed, added.get(p.key.as_str()).cloned().unwrap_or_default()));
    }
    // Who claims each guessed folder, for the shared ones.
    let mut owners: HashMap<String, Vec<&str>> = HashMap::new();
    for p in plugins {
        for g in claimed.get(p.key.as_str()).map(|c| c.0.as_slice()).unwrap_or_default() {
            let list = owners.entry(key(&g.path)).or_default();
            if !list.contains(&p.key.as_str()) {
                list.push(&p.key);
            }
        }
    }
    // Every folder counted on its own for some plugin (guessed for one, or added): a folder that holds one counts
    // without it, so nothing is counted twice.
    let mut counted: Vec<(String, &str)> = Vec::new();
    for (guessed, mine) in claimed.values() {
        for path in guessed.iter().map(|g| g.path.as_str()).chain(mine.iter().map(String::as_str)) {
            let k = key(path);
            if !counted.iter().any(|(c, _)| *c == k) {
                counted.push((k, path));
            }
        }
    }
    let mut own_size = |path: &str| -> Result<Measured, Stopped> {
        let mut all = size_of(path)?;
        let k = key(path);
        let inside: Vec<&(String, &str)> = counted.iter().filter(|(c, _)| strictly_inside(c, &k)).collect();
        // Only the outermost of those: one inside another of them is already in that one.
        for (c, inner) in inside.iter().filter(|(c, _)| !inside.iter().any(|(o, _)| strictly_inside(c, o))) {
            // Only one the walk of this folder reached (not past a link), or it isn't in this folder's size.
            if !plainly_inside(inner, c, path, &k) {
                continue;
            }
            let part = size_of(inner)?;
            all = Measured {
                bytes: all.bytes.saturating_sub(part.bytes),
                sound_bytes: all.sound_bytes.saturating_sub(part.sound_bytes),
                files: all.files.saturating_sub(part.files),
            };
        }
        Ok(all)
    };
    // The plugins that have a folder besides this one, by guess or because you added it.
    let others_of = |path: &str, me: &str| -> Vec<String> {
        let k = key(path);
        let mut others: Vec<String> = Vec::new();
        for o in owners.get(&k).into_iter().chain(added_by.get(&k)).flatten() {
            if *o != me && !others.iter().any(|x| x == o) {
                others.push((*o).to_owned());
            }
        }
        others
    };
    let mut result = Vec::new();
    for p in plugins.iter().filter(|p| only.is_none_or(|o| o.contains(&p.key))) {
        let mut folders: Vec<SpaceFolder> = Vec::new();
        // Each file once, by where it really is: a link to a file listed already isn't more room.
        let mut files_seen = HashSet::new();
        for f in p.files.iter().filter(|f| files_seen.insert(key(&real_path(f).unwrap_or_else(|| (*f).clone())))) {
            let m = own_size(f)?;
            folders.push(SpaceFolder {
                path: f.clone(),
                kind: SpaceKind::Program,
                bytes: m.bytes,
                shared_with: Vec::new(),
                added: false,
            });
        }
        let Some((guessed, mine)) = claimed.get(p.key.as_str()) else { continue };
        for g in guessed {
            // A maker's folder counts without the plugin folders inside it that are counted on their own (ones every
            // plugin ruled out stay part of the maker's folder, so they're counted somewhere).
            let m = own_size(&g.path)?;
            folders.push(SpaceFolder {
                path: g.path.clone(),
                kind: kind_of(m),
                bytes: m.bytes,
                shared_with: others_of(&g.path, &p.key),
                added: false,
            });
        }
        for a in mine {
            if folders.iter().any(|f| key(&f.path) == key(a)) {
                continue;
            }
            let m = own_size(a)?;
            folders.push(SpaceFolder {
                path: a.clone(),
                kind: kind_of(m),
                bytes: m.bytes,
                shared_with: others_of(a, &p.key),
                added: true,
            });
        }
        folders.retain(|f| f.bytes > 0 || f.added);
        result.push(PluginSpace { key: p.key.clone(), name: p.name.clone(), vendor: p.vendor.clone(), folders });
    }
    Ok(result)
}

/// Whether one path key is inside another (not the same).
fn strictly_inside(inner: &str, outer: &str) -> bool {
    inner.len() > outer.len()
        && inner.starts_with(outer)
        && (outer.ends_with('/') || inner.as_bytes().get(outer.len()) == Some(&b'/'))
}

/// Whether a folder inside another (`inner_key` inside `outer_key`) is reached from it without passing a link, as a
/// walk of the outer folder reaches it: where it really is is where the outer one really is, plus the same rest.
fn plainly_inside(inner: &str, inner_key: &str, outer: &str, outer_key: &str) -> bool {
    match (real_path(inner), real_path(outer)) {
        (Some(real_inner), Some(real_outer)) => {
            key(&real_inner) == format!("{}{}", key(&real_outer), &inner_key[outer_key.len()..])
        }
        _ => false,
    }
}

/// The installed plugins, from the plugin files in your plugin folders, grouped as the Plugins page groups them (a
/// plugin's VST2, VST3 and CLAP files together). The maker comes from what the plugin scanner read (`vendors`, by name
/// key), or else the folder the plugin sits in ("VST3\UVI\UVIWorkstation.vst3" is UVI's).
pub fn group(files: &[InstalledPlugin], plugin_roots: &[String], vendors: &HashMap<String, String>) -> Vec<Installed> {
    let mut out: Vec<Installed> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for f in files {
        let k = name_key(&f.name);
        match at.get(&k).and_then(|&i| out.get_mut(i)) {
            Some(installed) => installed.files.push(f.path.clone()),
            None => {
                at.insert(k.clone(), out.len());
                out.push(Installed {
                    key: k,
                    name: f.name.clone(),
                    vendor: String::new(),
                    files: vec![f.path.clone()],
                });
            }
        }
    }
    for installed in &mut out {
        installed.vendor = match vendors.get(&installed.key).filter(|v| !v.is_empty()) {
            Some(v) => v.clone(),
            None => installed
                .files
                .iter()
                .map(|f| folder_maker(f, plugin_roots))
                .find(|m| !m.is_empty())
                .unwrap_or_default(),
        };
    }
    out
}

/// The folder a plugin sits in right below its plugin folder, when that isn't a plugin itself: its maker.
fn folder_maker(path: &str, plugin_roots: &[String]) -> String {
    let parts = |p: &str| p.split(['\\', '/']).filter(|d| !d.is_empty()).map(str::to_owned).collect::<Vec<String>>();
    // The innermost plugin folder it's in.
    let Some(root) = plugin_roots.iter().filter(|r| is_inside(path, r)).max_by_key(|r| parts(r).len()) else {
        return String::new();
    };
    let all = parts(path);
    let rel = all.get(parts(root).len()..).unwrap_or_default();
    match rel {
        [first, _, ..] if !first.contains('.') => first.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_named_after_a_plugin_is_its() {
        assert!(name_matches("Serum Presets", "serum"));
        assert!(name_matches("Kontakt 8", "kontakt"));
        assert!(name_matches("Omnisphere Library", "omnisphere"));
        assert!(!name_matches("Serum Backups", "serum"));
        assert!(name_matches("M1", "m1"));
        assert!(!name_matches("M1 Presets", "m1"));
        assert!(!name_matches("Anything", "a"));
    }

    #[test]
    fn a_folder_named_after_a_maker_is_theirs() {
        assert!(maker_matches("Xfer", "xferrecords"));
        assert!(maker_matches("UVI", "uvi"));
        assert!(!maker_matches("UV", "uvi"));
        // "Arturia" for "Arturia SA", but not "Spit" for "Spitfire"'s digit-led neighbour.
        assert!(maker_matches("Arturia", "arturiasa"));
        assert!(!maker_matches("Korg", "korg2"));
    }

    #[test]
    fn the_maker_comes_from_the_folder_the_plugin_sits_in() {
        let roots = vec!["C:/VST3".to_owned()];
        assert_eq!(folder_maker("C:/VST3/UVI/UVIWorkstation.vst3", &roots), "UVI");
        assert_eq!(folder_maker("C:/VST3/Serum.vst3", &roots), "");
        assert_eq!(folder_maker("C:/VST3/Serum.vst3/Contents/x.vst3", &roots), "");
        assert_eq!(folder_maker("D:/Elsewhere/A/B.vst3", &roots), "");
        let nested = vec!["D:/Plugins".to_owned(), "D:/Plugins/VST3".to_owned()];
        assert_eq!(folder_maker("D:/Plugins/VST3/Serum.vst3", &nested), "");
        assert_eq!(folder_maker("D:/Plugins/VST3/UVI/Falcon.vst3", &nested), "UVI");
    }
}
