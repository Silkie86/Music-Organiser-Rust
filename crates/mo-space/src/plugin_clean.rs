//! Cleaning up a plugin's presets and sounds: the C# app's Clean up card on the Plugins page.
//!
//! Solomon's rules (25 Oct 2026): a plugin's sound library only goes when no project uses the plugin at all; its
//! presets may go once no project has used it for a while (the "unused" years you pick); everything under Program
//! Files is left to the plugin's own uninstaller. Nothing that holds your project or sample folders, a sample any
//! project uses, a plugin file, or a OneDrive file that's only online (the Recycle Bin can't keep those) ever goes.
//!
//! [`lines`] says, for each of a plugin's folders, whether it may go and why not; nothing changes. [`apply`] checks
//! each picked folder again just before it goes and sends it to the Recycle Bin, as [`crate::clean`] does for songs.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs::{self, Metadata};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use mo_formats::paths::file_name;
use mo_formats::textscan::is_audio_path;
use mo_library::scan::is_inside;
use rayon::prelude::*;

use crate::clean::{Bin, Done, Problem, Sending, Sent, What, both, on_network, parts};
use crate::plugin_space::{PluginSpace, SpaceFolder, SpaceKind, data_roots, is_install_folder};
use crate::plugins::is_plugin_file;
use crate::size::{DEEPEST, Measured, Stopped, is_online_only, is_sound};
use crate::{home_folder, key, real_path};

/// How your projects use a plugin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PluginUse {
    /// How many projects use it.
    pub projects: u32,
    /// Whether no project has used it for the years you picked (never used counts).
    pub unused: bool,
}

/// Why a plugin's folder stays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stays {
    /// It's under Program Files: Uninstall removes it.
    InstallFolder,
    /// It's inside one of your project or sample folders.
    InYourFolder(String),
    /// It holds one of your project or sample folders.
    HoldsYourFolder(String),
    /// A project uses a sample in it (one of its files' names).
    SampleInUse(String),
    /// Some of its files are only online in OneDrive: they can't go to the Recycle Bin.
    OnlineOnly,
    /// It's shared with these plugins (name keys), which your projects use.
    SharedWithUsed(Vec<String>),
    /// It's sounds, and projects use the plugin.
    SoundsInUse { projects: u32 },
    /// Your projects still use the plugin.
    StillUsed,
    /// It holds a plugin file (or bundle): Uninstall removes that.
    HoldsPluginFile(String),
    /// It holds another plugin's folder that stays: that folder, its plugin (name key), and why it stays.
    Holds { folder: String, plugin: String, why: Box<Stays> },
    /// It's a link to another place, or reached through one.
    Link,
    /// It's on a network drive, which has no Recycle Bin.
    NetworkDrive,
    /// It's a drive, your home folder, or a folder the computer keeps (Documents, Music, AppData…), or holds one.
    SystemFolder,
    /// Part of it can't be read (or is too deep to), so what's in it can't be checked.
    Unreadable(String),
    /// It isn't there any more.
    Gone,
    /// It changed since you looked (what's in it, or what the plugin's folders are).
    Changed,
}

impl Stays {
    /// What the app says. `name` is the plugin's name; `names` turns name keys into names.
    pub fn text(&self, name: &str, names: &dyn Fn(&str) -> String) -> String {
        match self {
            Stays::InstallFolder => "Inside Program Files: Uninstall removes this.".to_owned(),
            Stays::InYourFolder(f) => format!("Part of your folder {f}."),
            Stays::HoldsYourFolder(f) => format!("It holds your folder {f}."),
            Stays::SampleInUse(s) => format!("A project uses a sample in it ({s})."),
            Stays::OnlineOnly => "Some of its files are only online in OneDrive. They can't go to the Recycle Bin: \
                                  removing them deletes them from OneDrive everywhere."
                .to_owned(),
            Stays::SharedWithUsed(keys) => {
                format!(
                    "Shared with {}, which your projects use.",
                    keys.iter().map(|k| names(k)).collect::<Vec<_>>().join(", ")
                )
            }
            Stays::SoundsInUse { projects } => {
                let n = if *projects == 1 { "1 project does".to_owned() } else { format!("{projects} projects do") };
                format!("Sounds stay while a project uses {name}: {n}.")
            }
            Stays::StillUsed => format!("Your projects still use {name}."),
            Stays::HoldsPluginFile(f) => format!("It holds {}, a plugin file: Uninstall removes that.", file_name(f)),
            Stays::Holds { folder, plugin, why } => {
                format!("It holds {}, which stays. {}", file_name(folder), why.text(&names(plugin), names))
            }
            Stays::Link => "It's a link to another place, or reached through one.".to_owned(),
            Stays::NetworkDrive => "It's on a network drive, which has no Recycle Bin.".to_owned(),
            Stays::SystemFolder => "It's one of the computer's own folders, or holds one.".to_owned(),
            Stays::Unreadable(p) => format!("Part of it ({}) can't be read, so it can't be checked.", file_name(p)),
            Stays::Gone => "It isn't there any more.".to_owned(),
            Stays::Changed => "It changed since you looked, so it stays.".to_owned(),
        }
    }
}

/// What a folder held when it was looked at: its size, and a stamp of everything in it (where, how big, when written)
/// to see just before it goes whether anything in it changed, even a file renamed or saved again at the same size.
/// Files Finder and Explorer make by themselves when you open a folder (`.DS_Store`, `desktop.ini`, thumbnails) don't
/// count, and a folder counts by its name only (opening it can change its time).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Look {
    measured: Measured,
    stamp: u64,
}

impl Look {
    fn add(&mut self, other: &Look) {
        self.measured = self.measured + other.measured;
        self.stamp = self.stamp.wrapping_add(other.stamp);
    }

    /// Adds one file or folder to the stamp. Adding up each one's hash makes the stamp the same whatever order the
    /// walk met them in. The stamp is only compared within one run of the app, so Rust's own hash does.
    fn note(&mut self, path: &str, meta: &Metadata) {
        let mut h = DefaultHasher::new();
        path.hash(&mut h);
        if !meta.is_dir() {
            meta.len().hash(&mut h);
            meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).hash(&mut h);
        }
        self.stamp = self.stamp.wrapping_add(h.finish());
    }
}

/// Files the computer makes in a folder by itself when you look at it (Finder, Explorer).
pub(crate) fn made_by_the_system(name: &str) -> bool {
    const NAMES: [&str; 5] = [".DS_Store", "desktop.ini", "Thumbs.db", "ehthumbs.db", "Icon\r"];
    NAMES.iter().any(|n| n.eq_ignore_ascii_case(name)) || name.starts_with("._")
}

/// One of a plugin's folders on the Clean up card: whether it may go (and why not), and whether it's ticked at first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanLine {
    pub folder: SpaceFolder,
    pub stays: Option<Stays>,
    /// Ticked for you: sounds that may go. Presets wait for you to tick them.
    pub ticked: bool,
    /// What the folder held when it was looked at, to see at the end whether it changed.
    look: Look,
}

impl CleanLine {
    /// The room this line frees if it goes: the whole folder with everything in it (a maker's folder holds its
    /// plugins' folders too, though each plugin's size counts them once). For a folder that stays, the folder's own
    /// size. For what several lines free together, see [`freed`].
    pub fn bytes(&self) -> u64 {
        if self.stays.is_none() { self.look.measured.bytes } else { self.folder.bytes }
    }
}

/// The room the picked lines free together: each folder that may go once, and one inside another that goes counts
/// with it, as [`apply`] removes them.
pub fn freed(picked: &[CleanLine]) -> u64 {
    let mut seen = HashSet::new();
    let going: Vec<&CleanLine> =
        picked.iter().filter(|l| l.stays.is_none() && seen.insert(key(&l.folder.path))).collect();
    going
        .iter()
        .filter(|l| {
            !going
                .iter()
                .any(|o| key(&o.folder.path) != key(&l.folder.path) && is_inside(&l.folder.path, &o.folder.path))
        })
        .map(|l| l.bytes())
        .fold(0, u64::saturating_add)
}

/// What the library knows that Clean up must respect.
pub struct Library<'a> {
    /// Your project and sample folders.
    pub protected_roots: &'a [String],
    /// More folders that never go, nor any folder holding one, besides the computer's own (always kept).
    pub kept_folders: &'a [String],
    /// How your projects use each plugin, by name key. A plugin missing here counts as used.
    pub uses: &'a HashMap<String, PluginUse>,
    /// A sample a project uses that's stored inside this folder (by path), if any.
    pub sample_at: &'a (dyn Fn(&str) -> Option<String> + Sync),
    /// Whether any project uses an audio file of this name (projects keep the old PC's paths, so names count too).
    pub sample_named: &'a (dyn Fn(&str) -> bool + Sync),
    /// A plugin's name from its name key, for what the app says.
    pub names: &'a (dyn Fn(&str) -> String + Sync),
}

impl Library<'_> {
    fn no_projects(&self, key: &str) -> bool {
        self.uses.get(key).is_some_and(|u| u.projects == 0)
    }

    /// Why a folder stays because of how projects use the plugins it belongs to. A plugin the library doesn't know
    /// counts as used.
    fn in_use(&self, f: &SpaceFolder, plugin: &str) -> Option<Stays> {
        let used: Vec<String> = f.shared_with.iter().filter(|k| !self.no_projects(k)).cloned().collect();
        if !used.is_empty() {
            return Some(Stays::SharedWithUsed(used));
        }
        let use_ = self.uses.get(plugin).copied();
        let projects = use_.map_or(0, |u| u.projects);
        if f.kind == SpaceKind::Sounds && projects > 0 {
            return Some(Stays::SoundsInUse { projects });
        }
        if !use_.is_some_and(|u| u.unused) {
            return Some(Stays::StillUsed);
        }
        None
    }
}

/// The computer's own folders: your home folder and the folders in it the system keeps (Documents, Music, Desktop,
/// AppData…), the program and system folders, and where plugins keep their things ([`data_roots`]). A clean-up never
/// takes one, nor a folder holding one. On Windows it asks the system, once for each card or clean-up.
fn system_folders() -> Vec<String> {
    let home = home_folder();
    let mut out: Vec<PathBuf> = data_roots();
    out.extend(home.clone());
    let in_home: &[&str] = if cfg!(windows) {
        out.extend(windows_system_folders());
        &[]
    } else if cfg!(target_os = "macos") {
        out.extend(
            ["/Applications", "/Library", "/System", "/Users", "/Volumes", "/usr", "/opt", "/private"]
                .map(PathBuf::from),
        );
        out.extend(["/Library/Audio", "/Library/Audio/Plug-Ins"].map(PathBuf::from));
        &[
            "Desktop",
            "Downloads",
            "Movies",
            "Pictures",
            "Public",
            "Library",
            "Library/Audio",
            "Library/Audio/Plug-Ins",
            "Library/Preferences",
        ]
    } else {
        out.extend(
            ["/usr", "/opt", "/etc", "/home", "/mnt", "/media", "/var", "/tmp", "/srv", "/run"].map(PathBuf::from),
        );
        out.extend(["XDG_DATA_HOME", "XDG_CONFIG_HOME"].iter().filter_map(std::env::var_os).map(PathBuf::from));
        &["Desktop", "Downloads", "Music", "Pictures", "Videos", "Public", "Templates", ".local", ".cache"]
    };
    if let Some(h) = &home {
        out.extend(in_home.iter().map(|p| h.join(p)));
    }
    let mut seen = HashSet::new();
    out.into_iter().map(|p| p.to_string_lossy().into_owned()).filter(|p| !p.is_empty() && seen.insert(key(p))).collect()
}

/// The folders Windows keeps besides [`data_roots`]' (Documents, AppData, ProgramData, Program Files), wherever
/// you've moved them.
#[cfg(windows)]
fn windows_system_folders() -> Vec<PathBuf> {
    use known_folders::{KnownFolder, get_known_folder_path};
    [
        KnownFolder::Profile,
        KnownFolder::UserProfiles,
        KnownFolder::Desktop,
        KnownFolder::Downloads,
        KnownFolder::Music,
        KnownFolder::Pictures,
        KnownFolder::Videos,
        KnownFolder::SavedGames,
        KnownFolder::OneDrive,
        KnownFolder::SkyDrive,
        KnownFolder::Public,
        KnownFolder::PublicMusic,
        KnownFolder::Windows,
        KnownFolder::System,
        KnownFolder::Fonts,
    ]
    .into_iter()
    .filter_map(get_known_folder_path)
    .collect()
}

#[cfg(not(windows))]
fn windows_system_folders() -> Vec<PathBuf> {
    Vec::new()
}

/// Whether a folder is the top of a drive: another drive than the folder it's in (a disk mounted there), or the top
/// of the computer's own.
#[cfg(unix)]
fn is_drive(path: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let p = Path::new(path);
    match (fs::metadata(p), p.parent().map(fs::metadata)) {
        (Ok(here), Some(Ok(up))) => here.dev() != up.dev(),
        _ => true,
    }
}

/// On Windows a drive is a letter (`D:\`); a drive put into a folder is a link, which stays anyway.
#[cfg(not(unix))]
fn is_drive(_path: &str) -> bool {
    false
}

/// The drive a folder is on (Unix), to see a drive mounted inside a folder.
#[cfg(unix)]
fn device(meta: &Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.dev())
}

/// On Windows a drive put into a folder is a link, which isn't followed.
#[cfg(not(unix))]
fn device(_meta: &Metadata) -> Option<u64> {
    None
}

/// Something in a folder that keeps it, met on the walk.
enum Found {
    Plugin(String),
    Sample(String),
    Online,
    /// Another drive mounted in it (a disk or a network share): removing the folder would take what's on it.
    Drive,
    Unreadable(String),
}

impl Found {
    /// Which to tell first: a plugin file (Uninstall's), then a used sample, then OneDrive, then a part that can't be
    /// read; for the same kind, the first by name, so two looks say the same.
    fn order(&self) -> (u8, &str) {
        match self {
            Found::Plugin(p) => (0, p),
            Found::Sample(s) => (1, s),
            Found::Online => (2, ""),
            Found::Drive => (3, ""),
            Found::Unreadable(p) => (4, p),
        }
    }

    fn stays(self) -> Stays {
        match self {
            Found::Plugin(p) => Stays::HoldsPluginFile(p),
            Found::Sample(s) => Stays::SampleInUse(s),
            Found::Online => Stays::OnlineOnly,
            Found::Drive => Stays::SystemFolder,
            Found::Unreadable(p) => Stays::Unreadable(p),
        }
    }
}

/// What one folder holds, as the walk reads it.
#[derive(Default)]
struct Read {
    look: Look,
    found: Vec<Found>,
    dirs: Vec<PathBuf>,
}

/// The checks for one card or clean-up, with the folders they compare against worked out once.
struct Checker<'a> {
    library: &'a Library<'a>,
    all: &'a [PluginSpace],
    /// Your project and sample folders, as written and as they really are.
    protected: Vec<(String, String)>,
    /// The computer's own folders, as written and as they really are.
    system: Vec<(String, String)>,
}

impl<'a> Checker<'a> {
    fn new(library: &'a Library<'a>, all: &'a [PluginSpace]) -> Self {
        let pairs = |folders: &[String]| -> Vec<(String, String)> {
            folders
                .iter()
                .filter(|f| !f.is_empty())
                .map(|f| {
                    let (written, real) = both(f);
                    (written.to_owned(), real)
                })
                .collect()
        };
        let mut system = system_folders();
        system.extend(library.kept_folders.iter().cloned());
        Checker { library, all, protected: pairs(library.protected_roots), system: pairs(&system) }
    }

    /// Why a folder stays, if it must, and what it holds now. Where it is and how its plugins are used come first;
    /// only a folder that passes those is read.
    fn check(&self, f: &SpaceFolder, plugin: &str, stop: &AtomicBool) -> Result<(Option<Stays>, Look), Stopped> {
        let why = self.place(&f.path).or_else(|| self.library.in_use(f, plugin)).or_else(|| self.holds(f));
        if why.is_some() {
            return Ok((why, Look::default()));
        }
        self.survey(&f.path, stop)
    }

    /// Why a folder stays for what it is and where it is.
    fn place(&self, path: &str) -> Option<Stays> {
        let Ok(meta) = fs::symlink_metadata(path) else { return Some(Stays::Gone) };
        if meta.file_type().is_symlink() {
            return Some(Stays::Link);
        }
        let Some(real) = real_path(path) else { return Some(Stays::Gone) };
        if on_network(path) || on_network(&real) {
            return Some(Stays::NetworkDrive);
        }
        if is_install_folder(path) || is_install_folder(&real) {
            return Some(Stays::InstallFolder);
        }
        for (root, real_root) in &self.protected {
            if is_inside(path, root) || is_inside(&real, real_root) {
                return Some(Stays::InYourFolder(root.clone()));
            }
            if is_inside(root, path) || is_inside(real_root, &real) {
                return Some(Stays::HoldsYourFolder(root.clone()));
            }
        }
        // Reached through a link: what's removed isn't where it seems.
        if key(&real) != key(path) {
            return Some(Stays::Link);
        }
        let holds_system = self.system.iter().any(|(f, real_f)| is_inside(f, path) || is_inside(real_f, &real));
        if parts(path).count() < 2 || is_drive(&real) || holds_system {
            return Some(Stays::SystemFolder);
        }
        (self.library.sample_at)(path).map(Stays::SampleInUse)
    }

    /// Why a folder stays because of the plugins' folders listed inside it: removing it removes them.
    fn holds(&self, f: &SpaceFolder) -> Option<Stays> {
        for s in self.all {
            for g in s.folders.iter().filter(|g| is_inside(&g.path, &f.path) && !is_inside(&f.path, &g.path)) {
                if g.kind == SpaceKind::Program {
                    return Some(Stays::HoldsPluginFile(g.path.clone()));
                }
                if let Some(why) = self.library.in_use(g, &s.key) {
                    return Some(Stays::Holds { folder: g.path.clone(), plugin: s.key.clone(), why: Box::new(why) });
                }
            }
        }
        None
    }

    /// Reads everything in a folder, a level at a time over every core: what it holds now, and why it stays for
    /// what's in it (a plugin file or bundle, a sample a project uses by name, a OneDrive file that's only online, a
    /// part that can't be read). It stops at the first level where it finds one.
    fn survey(&self, folder: &str, stop: &AtomicBool) -> Result<(Option<Stays>, Look), Stopped> {
        let mut look = Look::default();
        let mut level = vec![PathBuf::from(folder)];
        for _ in 0..DEEPEST {
            if level.is_empty() {
                break;
            }
            if stop.load(Ordering::Relaxed) {
                return Err(Stopped);
            }
            let reads: Vec<Read> = level.par_iter().map(|d| self.read_folder(d, stop)).collect();
            if stop.load(Ordering::Relaxed) {
                return Err(Stopped);
            }
            level = Vec::new();
            let mut found = Vec::new();
            for r in reads {
                look.add(&r.look);
                found.extend(r.found);
                level.extend(r.dirs);
            }
            if let Some(first) = found.into_iter().min_by(|a, b| a.order().cmp(&b.order())) {
                return Ok((Some(first.stays()), look));
            }
        }
        // Deeper than any real folder: what's down there isn't checked, so it stays.
        if let Some(deep) = level.first() {
            return Ok((Some(Stays::Unreadable(deep.to_string_lossy().into_owned())), look));
        }
        Ok((None, look))
    }

    /// One folder's files and folders, for [`Checker::survey`]. A link is noted (it goes with the folder; what it
    /// leads to stays where it is) but not followed, unless it's to a plugin.
    fn read_folder(&self, dir: &Path, stop: &AtomicBool) -> Read {
        let mut read = Read::default();
        // Stop is looked at before each folder is opened and every 256 entries; the survey then says it stopped.
        if stop.load(Ordering::Relaxed) {
            return read;
        }
        let here = dir.to_string_lossy();
        let (Ok(entries), Ok(dir_meta)) = (fs::read_dir(dir), fs::metadata(dir)) else {
            read.found.push(Found::Unreadable(here.into_owned()));
            return read;
        };
        let drive = device(&dir_meta);
        for (n, entry) in entries.enumerate() {
            if n % 256 == 255 && stop.load(Ordering::Relaxed) {
                break;
            }
            let Ok(entry) = entry else {
                read.found.push(Found::Unreadable(here.clone().into_owned()));
                continue;
            };
            let path = entry.path();
            let text = path.to_string_lossy().into_owned();
            // On Windows these come from the folder listing itself, so a file that's only online isn't opened.
            let (Ok(kind), Ok(meta)) = (entry.file_type(), entry.metadata()) else {
                read.found.push(Found::Unreadable(text));
                continue;
            };
            if kind.is_file() && made_by_the_system(file_name(&text)) {
                continue;
            }
            read.look.note(&text, &meta);
            // A plugin's file, or its bundle (a folder on a Mac, and a .vst3 on Windows), or a link to one.
            if is_plugin_file(&text) {
                read.found.push(Found::Plugin(text));
                continue;
            }
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if device(&meta) != drive {
                    read.found.push(Found::Drive);
                } else {
                    read.dirs.push(path);
                }
                continue;
            }
            if is_online_only(&meta) {
                read.found.push(Found::Online);
                continue;
            }
            let name = file_name(&text);
            if is_audio_path(&text) && (self.library.sample_named)(name) {
                read.found.push(Found::Sample(name.to_owned()));
            }
            let len = meta.len();
            let sound_bytes = if is_sound(&text, len) { len } else { 0 };
            read.look.measured = read.look.measured + Measured { bytes: len, sound_bytes, files: 1 };
        }
        read
    }
}

/// The Clean up card's lines for a plugin: each of its preset and sound folders (not its plugin files), biggest
/// first, with why it stays when it must. `all` is every plugin's measured space, to see what each folder holds.
/// Reads each folder that may go, to look for samples projects use: call it away from the window's thread.
pub fn lines(
    space: &PluginSpace,
    all: &[PluginSpace],
    library: &Library<'_>,
    stop: &AtomicBool,
) -> Result<Vec<CleanLine>, Stopped> {
    let checker = Checker::new(library, all);
    let mut folders: Vec<&SpaceFolder> = space.folders.iter().filter(|f| f.kind != SpaceKind::Program).collect();
    folders.sort_by_key(|f| std::cmp::Reverse(f.bytes));
    let mut out = Vec::new();
    for f in folders {
        let (stays, look) = checker.check(f, &space.key, stop)?;
        out.push(CleanLine { ticked: stays.is_none() && f.kind == SpaceKind::Sounds, folder: f.clone(), stays, look });
    }
    Ok(out)
}

/// Sends the picked lines that may go to the Recycle Bin, checking each again just before: where it is, how the
/// plugins are used now, what's in it, and that it holds just what it held when you looked. Lines that must stay are
/// never sent. `space` is the plugin's measured space and `all` every plugin's, as new as you have them: a line that
/// isn't one of `space`'s folders any more stays. A line picked twice counts once, and a folder inside one that goes
/// goes with it.
pub fn apply(
    picked: &[CleanLine],
    space: &PluginSpace,
    all: &[PluginSpace],
    library: &Library<'_>,
    bin: &dyn Bin,
    stop: &AtomicBool,
) -> Done {
    let checker = Checker::new(library, all);
    let mut done = Done::default();
    let mut seen = HashSet::new();
    let mut todo: Vec<&CleanLine> = picked.iter().filter(|l| seen.insert(key(&l.folder.path))).collect();
    // Outer folders first, so one inside a folder that goes is seen to have gone with it.
    todo.sort_by_key(|l| parts(&l.folder.path).count());
    // Every check again, as just before it goes: why it stays, if it does.
    let check = |line: &CleanLine| -> Result<(), String> {
        let path = line.folder.path.as_str();
        let now = space.folders.iter().find(|f| key(&f.path) == key(path) && f.kind == line.folder.kind);
        let why = match (&line.stays, now) {
            (Some(why), _) => why.clone(),
            (None, None) => Stays::Changed,
            (None, Some(now)) => match checker.check(now, &space.key, stop) {
                Err(Stopped) => return Err("Stopped before it went.".to_owned()),
                Ok((Some(why), _)) => why,
                Ok((None, look)) if look == line.look => return Ok(()),
                Ok((None, _)) => Stays::Changed,
            },
        };
        Err(why.text(&space.name, library.names))
    };
    let check = &check;
    let mut sending = Sending::new(bin, stop);
    // Rounds: a folder inside one waiting for Windows' administrator question waits for how that goes. If the outer one
    // goes, it went with it; if it stays (you said no), it's tried on its own in the next round, and if Windows guards
    // it too it stays as well: Windows isn't asked twice in one clean-up.
    while !todo.is_empty() {
        let (mut went, mut waits): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
        let mut inside_waiting = Vec::new();
        for line in todo {
            let path = line.folder.path.as_str();
            if let Some(outer) = went.iter().find(|g| is_inside(path, g)) {
                done.went_with.push((path.to_owned(), (*outer).to_owned()));
                continue;
            }
            if let Some(outer) = waits.iter().find(|g| is_inside(path, g)) {
                inside_waiting.push((line, *outer));
                continue;
            }
            if let Err(why) = check(line) {
                done.problems.push(Problem { path: path.to_owned(), why });
                continue;
            }
            let real = real_path(path).unwrap_or_else(|| path.to_owned());
            let kind = What::Plugin(line.folder.kind);
            match sending.send(path, real, line.look.measured.bytes, kind, &mut done, move || check(line)) {
                Sent::Went => went.push(path),
                Sent::Waits => waits.push(path),
                Sent::Stays => {}
            }
        }
        sending.flush(&mut done);
        todo = Vec::new();
        for (line, outer) in inside_waiting {
            if done.removed.iter().any(|r| key(&r.path) == key(outer)) {
                done.went_with.push((line.folder.path.clone(), outer.to_owned()));
            } else {
                todo.push(line);
            }
        }
    }
    sending.finish(&mut done);
    done
}
