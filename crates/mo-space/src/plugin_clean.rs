//! Cleaning up a plugin's presets and sounds: the C# app's Clean up card on the Plugins page.
//!
//! Solomon's rules (25 Oct 2026): a plugin's sound library only goes when no project uses the plugin at all; its
//! presets may go once no project has used it for a while (the "unused" years you pick); everything under Program
//! Files is left to the plugin's own uninstaller. Nothing that holds your project or sample folders, a sample any
//! project uses, a plugin file, or a OneDrive file that's only online (the Recycle Bin can't keep those) ever goes.
//!
//! [`lines`] says, for each of a plugin's folders, whether it may go and why not; nothing changes. [`apply`] checks
//! each picked folder again just before it goes and sends it to the Recycle Bin, as [`crate::clean`] does for songs.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use mo_formats::paths::file_name;
use mo_formats::textscan::is_audio_path;
use mo_library::scan::is_inside;

use crate::clean::{Bin, Done, Problem, What, both, on_network, send};
use crate::plugin_space::{PluginSpace, SpaceFolder, SpaceKind, data_roots, is_install_folder};
use crate::plugins::is_plugin_file;
use crate::size::{self, Measured, Stopped};
use crate::{key, real_path};

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
    /// It's part of one of your project or sample folders, or holds one.
    YourFolder(String),
    /// A project uses a sample in it (one of its files' names).
    SampleInUse(String),
    /// Some of its files are only online in OneDrive: they can't go to the Recycle Bin.
    OnlineOnly,
    /// It's shared with these plugins (name keys), and your projects use one of them.
    SharedWithUsed(Vec<String>),
    /// It's sounds, and projects use the plugin (or one it shares the folder with).
    SoundsInUse { projects: u32 },
    /// Your projects still use the plugin.
    StillUsed,
    /// It holds a plugin file: Uninstall removes that.
    HoldsPluginFile(String),
    /// It holds another plugin's folder that stays.
    Holds { folder: String, why: Box<Stays> },
    /// It's a link to another place, or reached through one.
    Link,
    /// It's on a network drive, which has no Recycle Bin.
    NetworkDrive,
    /// It's a drive, your home folder, or a folder Windows keeps (Documents, AppData…), or holds one.
    SystemFolder,
    /// It isn't there any more.
    Gone,
    /// It changed since you looked.
    Changed,
}

impl Stays {
    /// What the app says. `name` is the plugin's name; `names` turns name keys into names.
    pub fn text(&self, name: &str, names: &dyn Fn(&str) -> String) -> String {
        match self {
            Stays::InstallFolder => "Inside Program Files: Uninstall removes this.".to_owned(),
            Stays::YourFolder(f) => format!("Part of your folder {f}."),
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
            Stays::Holds { folder, why } => {
                format!("It holds {}, which stays. {}", file_name(folder), why.text(name, names))
            }
            Stays::Link => "It's a link to another place, or reached through one.".to_owned(),
            Stays::NetworkDrive => "It's on a network drive, which has no Recycle Bin.".to_owned(),
            Stays::SystemFolder => "It's one of the computer's own folders, or holds one.".to_owned(),
            Stays::Gone => "It isn't there any more.".to_owned(),
            Stays::Changed => "It changed since you looked, so it stays.".to_owned(),
        }
    }
}

/// One of a plugin's folders on the Clean up card: whether it may go (and why not), and whether it's ticked at first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanLine {
    pub folder: SpaceFolder,
    pub stays: Option<Stays>,
    /// Ticked for you: sounds that may go. Presets wait for you to tick them.
    pub ticked: bool,
    /// What the folder held when it was looked at, to see at the end whether it changed.
    measured: Measured,
}

/// What the library knows that Clean up must respect.
pub struct Library<'a> {
    /// Your project and sample folders.
    pub protected_roots: &'a [String],
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

    /// Why a folder stays because of how projects use the plugins it belongs to.
    fn in_use(&self, f: &SpaceFolder, plugin: &str) -> Option<Stays> {
        let use_ = self.uses.get(plugin).copied();
        let shared_used = !f.shared_with.iter().all(|k| self.no_projects(k));
        if f.shared() && shared_used {
            return Some(Stays::SharedWithUsed(f.shared_with.clone()));
        }
        let projects = use_.map_or(0, |u| u.projects);
        if f.kind == SpaceKind::Sounds && (projects > 0 || shared_used) {
            return Some(Stays::SoundsInUse { projects });
        }
        if !use_.is_some_and(|u| u.unused) {
            return Some(Stays::StillUsed);
        }
        None
    }

    /// Why a folder stays because of the folders listed inside it: removing it removes them.
    fn holds(&self, f: &SpaceFolder, all: &[PluginSpace]) -> Option<Stays> {
        for s in all {
            for g in s.folders.iter().filter(|g| is_inside(&g.path, &f.path) && !is_inside(&f.path, &g.path)) {
                if g.kind == SpaceKind::Program {
                    return Some(Stays::HoldsPluginFile(g.path.clone()));
                }
                // A plugin the library doesn't know counts as used.
                let why = if self.uses.contains_key(&s.key) { self.in_use(g, &s.key) } else { Some(Stays::StillUsed) };
                if let Some(why) = why {
                    return Some(Stays::Holds { folder: g.path.clone(), why: Box::new(why) });
                }
            }
        }
        None
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
        for (root, real_root) in self.protected_roots.iter().map(|r| both(r)) {
            if is_inside(path, root)
                || is_inside(root, path)
                || is_inside(&real, &real_root)
                || is_inside(&real_root, &real)
            {
                return Some(Stays::YourFolder(root.to_owned()));
            }
        }
        // Reached through a link: what's removed isn't where it seems.
        if key(&real) != key(path) {
            return Some(Stays::Link);
        }
        // A drive, your home folder, or a folder Windows keeps (Documents, AppData, Program Files…), or one holding one.
        let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
        let system = data_roots().into_iter().map(|r| r.to_string_lossy().into_owned()).chain([home]);
        let parts = path.split(['\\', '/']).filter(|d| !d.is_empty()).count();
        if parts < 2 || system.filter(|f| !f.is_empty()).any(|f| is_inside(&f, path) || is_inside(&f, &real)) {
            return Some(Stays::SystemFolder);
        }
        (self.sample_at)(path).map(Stays::SampleInUse)
    }

    /// Why a folder stays for what's in it: a sample a project uses by name, a plugin file, a file only online.
    fn contents(&self, path: &str, stop: &AtomicBool) -> Result<Option<Stays>, Stopped> {
        enum Found {
            Sample(String),
            Plugin(String),
            Online,
        }
        let found = size::walk(Path::new(path), true, stop, |p, meta| {
            let text = p.to_string_lossy();
            if size::is_online_only(meta) {
                Some(Found::Online)
            } else if is_plugin_file(&text) {
                Some(Found::Plugin(text.into_owned()))
            } else if is_audio_path(&text) && (self.sample_named)(file_name(&text)) {
                Some(Found::Sample(file_name(&text).to_owned()))
            } else {
                None
            }
        })?;
        // A plugin file first (Uninstall's), then a used sample, then OneDrive.
        let plugin = found.iter().find_map(|f| if let Found::Plugin(p) = f { Some(p.clone()) } else { None });
        let sample = found.iter().find_map(|f| if let Found::Sample(s) = f { Some(s.clone()) } else { None });
        Ok(plugin
            .map(Stays::HoldsPluginFile)
            .or(sample.map(Stays::SampleInUse))
            .or(found.iter().any(|f| matches!(f, Found::Online)).then_some(Stays::OnlineOnly)))
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
    let mut folders: Vec<&SpaceFolder> = space.folders.iter().filter(|f| f.kind != SpaceKind::Program).collect();
    folders.sort_by_key(|f| std::cmp::Reverse(f.bytes));
    let mut out = Vec::new();
    for f in folders {
        let mut stays =
            library.place(&f.path).or_else(|| library.in_use(f, &space.key)).or_else(|| library.holds(f, all));
        if stays.is_none() {
            stays = library.contents(&f.path, stop)?;
        }
        let measured = if stays.is_none() { size::measure(Path::new(&f.path), stop)? } else { Measured::default() };
        out.push(CleanLine {
            ticked: stays.is_none() && f.kind == SpaceKind::Sounds,
            folder: f.clone(),
            stays,
            measured,
        });
    }
    Ok(out)
}

/// Sends the picked lines that may go to the Recycle Bin, checking each again just before: where it is, what's in it,
/// and that it holds what it held when you looked. Lines that must stay are never sent. `space` is the plugin's
/// measured space and `all` every plugin's, as given to [`lines`].
pub fn apply(
    picked: &[CleanLine],
    space: &PluginSpace,
    all: &[PluginSpace],
    library: &Library<'_>,
    bin: &dyn Bin,
    stop: &AtomicBool,
) -> Done {
    let mut done = Done::default();
    for line in picked {
        let path = line.folder.path.as_str();
        let stays = match line.stays.clone() {
            Some(why) => Some(why),
            None => {
                let again = library
                    .place(path)
                    .or_else(|| library.in_use(&line.folder, &space.key))
                    .or_else(|| library.holds(&line.folder, all));
                match again {
                    Some(why) => Some(why),
                    None => match library.contents(path, stop) {
                        Ok(Some(why)) => Some(why),
                        Ok(None) => match size::measure(Path::new(path), stop) {
                            Ok(m) if m == line.measured => None,
                            Ok(_) => Some(Stays::Changed),
                            Err(Stopped) => {
                                done.problems
                                    .push(Problem { path: path.to_owned(), why: "Stopped before it went.".into() });
                                continue;
                            }
                        },
                        Err(Stopped) => {
                            done.problems
                                .push(Problem { path: path.to_owned(), why: "Stopped before it went.".into() });
                            continue;
                        }
                    },
                }
            }
        };
        if let Some(why) = stays {
            done.problems.push(Problem { path: path.to_owned(), why: why.text(&space.name, library.names) });
            continue;
        }
        let real = real_path(path).unwrap_or_else(|| path.to_owned());
        send(path, real, line.folder.bytes, What::Plugin(line.folder.kind), bin, &mut done);
    }
    let not_found = bin.find(&mut done.removed);
    done.problems.extend(not_found);
    done
}
