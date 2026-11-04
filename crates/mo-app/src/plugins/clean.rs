//! Clean up on the Plugins page: which of the picked plugin's preset and sound folders may go to the Recycle Bin, in
//! words for its card. The checks themselves are `mo_space::plugin_clean`'s (Solomon's rules of 25 Oct 2026: a
//! plugin's sounds only go when no project uses it at all, its presets once none has for the years you pick, and
//! nothing under Program Files, nothing of yours, no sample a project uses). This only gathers what they need to know
//! of your library and says what they found.

use std::collections::HashMap;

use mo_library::plugins::SampleIndex;
use mo_library::scan::is_inside;
use mo_library::{Library, LibraryError};
use mo_space::clean::Done;
use mo_space::key;
use mo_space::plugin_clean::{self, CleanLine, PluginUse as CleanUse};
use mo_space::plugin_space::{PluginSpace, SpaceKind};
use mo_space::size::size_text;

use super::{Plugins, plural, unused};

/// What Clean up looks at for one plugin: its room, everyone's (to see what each folder holds), and how your projects
/// use each plugin. Made on the window's thread from the page; the library's part is read on a worker
/// ([`Ask::read`]).
#[derive(Clone, Debug)]
pub struct Ask {
    /// The row it's for, and its name key and own (to find it again after a new read).
    pub row: usize,
    pub row_key: (String, String),
    pub name: String,
    /// How many saves use it.
    pub projects: usize,
    pub space: PluginSpace,
    pub all: Vec<PluginSpace>,
    uses: HashMap<String, CleanUse>,
    names: HashMap<String, String>,
}

/// Everything Clean up checks a folder against.
#[derive(Clone, Debug)]
pub struct Inputs {
    pub ask: Ask,
    /// Your project and sample folders.
    pub protected: Vec<String>,
    /// More folders that never go (the app's own, your MIDI and plugin folders).
    pub kept: Vec<String>,
    pub samples: SampleIndex,
}

impl Ask {
    /// Reads what the library knows that Clean up must respect: your folders and every sample a project uses. Slow
    /// on a big library: call it away from the window's thread.
    pub fn read(self, lib: &Library, kept: Vec<String>) -> Result<Inputs, LibraryError> {
        let mut kept = kept;
        kept.extend(lib.list_setting("midi_folders")?);
        kept.extend(lib.list_setting("plugin_folders")?);
        kept.retain(|k| !k.trim().is_empty());
        Ok(Inputs { protected: lib.protected_roots()?, kept, samples: lib.sample_index()?, ask: self })
    }
}

impl Inputs {
    /// Runs `f` with these as `plugin_clean`'s view of the library.
    pub fn with<R>(&self, f: impl FnOnce(&plugin_clean::Library<'_>) -> R) -> R {
        let sample_at = |p: &str| self.samples.inside(p);
        let sample_named = |n: &str| self.samples.named(n);
        let names = |k: &str| self.name_of(k);
        let lib = plugin_clean::Library {
            protected_roots: &self.protected,
            kept_folders: &self.kept,
            uses: &self.ask.uses,
            sample_at: &sample_at,
            sample_named: &sample_named,
            names: &names,
        };
        f(&lib)
    }

    /// A plugin's name from its name key (the key itself when it isn't known).
    pub fn name_of(&self, key: &str) -> String {
        self.ask.names.get(key).cloned().unwrap_or_else(|| key.to_owned())
    }
}

impl Plugins {
    /// What Clean up needs for the plugin at `row`, or `None` when there's nothing to clean: not installed, or the
    /// old app found no folders of its own for it.
    pub fn clean_ask(&self, row: usize) -> Option<Ask> {
        let r = self.rows.get(row)?;
        let space = r.room.clone().filter(|_| r.installed)?;
        let mut uses: HashMap<String, CleanUse> = HashMap::new();
        let mut names: HashMap<String, String> = self.spaces.iter().map(|s| (s.key.clone(), s.name.clone())).collect();
        // By the measured plugin's key, which may differ from the projects' name for it. A measured plugin with no row
        // (one claimed by a row that has its own) isn't listed, so it counts as used.
        for r in &self.rows {
            if let Some(room) = r.room.as_ref().filter(|_| r.installed) {
                let projects = u32::try_from(r.saves.len()).unwrap_or(u32::MAX);
                uses.insert(room.key.clone(), CleanUse { projects, unused: r.unused });
                names.insert(room.key.clone(), r.name.clone());
            }
        }
        Some(Ask {
            row,
            row_key: (r.key.clone(), r.own.clone()),
            name: r.name.clone(),
            projects: r.saves.len(),
            space,
            all: self.spaces.clone(),
            uses,
            names,
        })
    }

    /// Leaves out what a clean-up sent to the Recycle Bin (and what went inside it), until the old app measures again.
    pub fn forget_removed(&mut self, done: &Done) {
        let gone: Vec<&str> = done
            .removed
            .iter()
            .map(|r| r.path.as_str())
            .chain(done.went_with.iter().map(|(p, _)| p.as_str()))
            .collect();
        if gone.is_empty() {
            return;
        }
        let keep = |path: &str| !gone.iter().any(|g| key(g) == key(path) || is_inside(path, g));
        for s in &mut self.spaces {
            s.folders.retain(|f| keep(&f.path));
        }
        let cutoff = self.cutoff();
        for r in &mut self.rows {
            if let Some(room) = r.room.as_mut() {
                room.folders.retain(|f| keep(&f.path));
            }
            r.unused = unused(r, cutoff);
        }
        self.refilter();
    }
}

/// One line of the Clean up card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanRow {
    pub path: String,
    /// "Presets and settings" or "Sounds".
    pub kind: &'static str,
    pub size: String,
    /// Why it stays ("" when it may go).
    pub why: String,
    pub can: bool,
    /// Ticked at first.
    pub ticked: bool,
}

/// The card's lines, as `plugin_clean::lines` found them.
pub fn rows(lines: &[CleanLine], inputs: &Inputs) -> Vec<CleanRow> {
    let names = |k: &str| inputs.name_of(k);
    lines
        .iter()
        .map(|l| CleanRow {
            path: l.folder.path.clone(),
            kind: match l.folder.kind {
                SpaceKind::Program => "Plugin file",
                SpaceKind::Presets => "Presets and settings",
                SpaceKind::Sounds => "Sounds",
            },
            size: size_text(l.bytes()),
            why: l.stays.as_ref().map(|w| w.text(&inputs.ask.name, &names)).unwrap_or_default(),
            can: l.stays.is_none(),
            ticked: l.ticked,
        })
        .collect()
}

/// The lines ticked (only those that may go).
pub fn picked(lines: &[CleanLine], ticks: &[bool]) -> Vec<CleanLine> {
    lines.iter().zip(ticks).filter(|(l, t)| **t && l.stays.is_none()).map(|(l, _)| l.clone()).collect()
}

/// "Frees 4.2 GB", or what to do when nothing is ticked.
pub fn freed_text(lines: &[CleanLine], ticks: &[bool]) -> String {
    let picked = picked(lines, ticks);
    if picked.is_empty() {
        if lines.iter().any(|l| l.stays.is_none()) {
            "Tick what to send to the Recycle Bin.".to_owned()
        } else {
            "Nothing here can go: each line says why.".to_owned()
        }
    } else {
        format!("Frees {}", size_text(plugin_clean::freed(&picked)))
    }
}

/// What the card says over its lines.
pub fn intro(inputs: &Inputs, lines: &[CleanLine]) -> String {
    if lines.is_empty() {
        return format!(
            "No presets or sound folders were found for {}. Uninstall removes its plugin files.",
            inputs.ask.name
        );
    }
    let mut s = "Tick what to remove. Everything goes to the Recycle Bin, so you can still put it back.".to_owned();
    if inputs.ask.projects > 0 {
        s.push_str(&format!(
            " {} {}. Its presets are safe to remove: projects keep their own settings.",
            if inputs.ask.projects == 1 {
                "1 project uses".to_owned()
            } else {
                format!("{} projects use", inputs.ask.projects)
            },
            inputs.ask.name
        ));
    }
    s
}

/// What a clean-up did: "Freed 4.2 GB. 2 folders are in the Recycle Bin.", and each problem.
pub fn done_text(done: &Done) -> (String, Vec<String>) {
    let went = done.removed.len();
    let head = if went == 0 {
        "Nothing was removed.".to_owned()
    } else {
        format!(
            "Freed {}. {} in the Recycle Bin, so you can put {} back.",
            size_text(done.bytes()),
            if went == 1 { "1 folder is".to_owned() } else { format!("{went} folders are") },
            if went == 1 { "it" } else { "them" }
        )
    };
    let problems = done.problems.iter().map(|p| format!("{}: {}", p.path, p.why)).collect();
    (head, problems)
}

/// What putting back said: "Put back 2 folders.", and each problem.
pub fn undone_text(done: &Done, problems: &[mo_space::clean::Problem]) -> (String, Vec<String>) {
    let back = done.removed.len().saturating_sub(problems.len());
    (
        if back == 0 {
            "Nothing was put back.".to_owned()
        } else {
            format!("Put back {}.", plural(back, "folder", "folders"))
        },
        problems.iter().map(|p| format!("{}: {}", p.path, p.why)).collect(),
    )
}
