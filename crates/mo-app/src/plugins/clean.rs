//! Clean up on the Plugins page: which of the picked plugin's preset and sound folders may go to the Recycle Bin, in
//! words for its card. The checks themselves are `mo_space::plugin_clean`'s (Solomon's rules of 25 Oct 2026: a
//! plugin's sounds only go when no project uses it at all, its presets once none has for the years you pick, and
//! nothing under Program Files, nothing of yours, no sample a project uses). This only gathers what they need to know
//! of your library and says what they found.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use mo_library::plugins::SampleIndex;
use mo_library::{Library, LibraryError};
use mo_space::admin::Admin;
use mo_space::clean::{Bin, Done, NOT_IN_BIN, Problem, Removed, Room, What};
use mo_space::key;
use mo_space::plugin_clean::{self, CleanLine, PluginUse as CleanUse};
use mo_space::plugin_space::{PluginSpace, SpaceKind};
use mo_space::size::size_text;

use super::{Plugins, plural};

/// What Clean up looks at for one plugin: its room, everyone's (to see what each folder holds), and how your projects
/// use each plugin. Made on the window's thread from the page; the library's part is read on a worker
/// ([`Ask::read`]).
#[derive(Clone, Debug)]
pub struct Ask {
    /// The row it's for, and its name key and own (to find it again after a new read).
    pub row: usize,
    pub row_key: (String, String),
    pub name: String,
    /// How many songs use it (any save), as its card's Used tab counts them (review E2-10).
    pub songs: usize,
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
    /// on a big library: call it away from the window's thread. Your MIDI folders are yours as your project and
    /// sample folders are: nothing inside one goes either. A folder setting that can't be read is an error, never
    /// read as no folders.
    pub fn read(self, lib: &Library, kept: Vec<String>) -> Result<Inputs, LibraryError> {
        let mut protected = lib.protected_roots()?;
        protected.extend(lib.strict_list_setting("midi_folders")?);
        protected.retain(|k| !k.trim().is_empty());
        let mut kept = kept;
        kept.extend(lib.strict_list_setting("plugin_folders")?);
        kept.retain(|k| !k.trim().is_empty());
        Ok(Inputs { protected, kept, samples: lib.sample_index()?, ask: self })
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
    /// Whether the plugin at `row` has anything Clean up could look at: installed, with a preset or sound folder the
    /// last measure has (its plugin files are Uninstall's).
    pub fn can_clean(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|r| {
            r.installed && r.room.as_ref().is_some_and(|s| s.folders.iter().any(|f| f.kind != SpaceKind::Program))
        })
    }

    /// Each measured plugin's name by its name key: the page's name for it where an installed row has its room.
    pub(super) fn plugin_names(&self) -> HashMap<String, String> {
        let mut names: HashMap<String, String> = self.spaces.iter().map(|s| (s.key.clone(), s.name.clone())).collect();
        for r in &self.rows {
            if let Some(room) = r.room.as_ref().filter(|_| r.installed) {
                names.insert(room.key.clone(), r.name.clone());
            }
        }
        names
    }

    /// The row with this name key and own, now.
    pub fn row_of(&self, row_key: &(String, String)) -> Option<usize> {
        self.rows.iter().position(|r| r.key == row_key.0 && r.own == row_key.1)
    }

    /// What Clean up needs for the plugin at `row`, or `None` when there's nothing to clean ([`Plugins::can_clean`]).
    pub fn clean_ask(&self, row: usize) -> Option<Ask> {
        if !self.can_clean(row) {
            return None;
        }
        let r = self.rows.get(row)?;
        let space = r.room.clone()?;
        let mut uses: HashMap<String, CleanUse> = HashMap::new();
        let names = self.plugin_names();
        // By the measured plugin's key, which may differ from the projects' name for it. A measured plugin with no row
        // (one claimed by a row that has its own) isn't listed, so it counts as used.
        for r in &self.rows {
            if let Some(room) = r.room.as_ref().filter(|_| r.installed) {
                // A stand-in for a missing version counts as used by that version's songs (review E2-6).
                let saves = if r.stands_in.is_some() { r.saves.len().max(1) } else { r.saves.len() };
                let projects = u32::try_from(saves).unwrap_or(u32::MAX);
                uses.insert(room.key.clone(), CleanUse { projects, unused: r.unused });
            }
        }
        Some(Ask {
            row,
            row_key: (r.key.clone(), r.own.clone()),
            name: r.name.clone(),
            songs: r.songs,
            space,
            all: self.spaces.clone(),
            uses,
            names,
        })
    }

    /// Leaves out what a clean-up sent to the Recycle Bin (and what went inside it), until the next measure.
    pub fn forget_removed(&mut self, done: &Done) {
        // What went with another went inside it, so the folders sent cover it.
        let gone: Vec<String> = done.removed.iter().map(|r| r.path.clone()).collect();
        self.forget_paths(&gone);
    }
}

/// What of a clean-up really is in your Recycle Bin, for Put back: what went but wasn't found in it (removed outright,
/// [`NOT_IN_BIN`], or sent as administrator to another account's bin) has a problem of its own path, can't be put back,
/// and isn't offered (review E2-7), nor what went inside it. Problems about what stayed are kept out too: they were
/// said already. Leave out [`removed_for_good`]'s paths too, apart.
pub fn in_the_bin(done: &Done) -> Done {
    let not_found = |path: &str| done.problems.iter().any(|p| p.path == path);
    let removed: Vec<Removed> = done.removed.iter().filter(|r| !not_found(&r.path)).cloned().collect();
    let went_with =
        done.went_with.iter().filter(|(_, outer)| removed.iter().any(|r| &r.path == outer)).cloned().collect();
    Done { removed, problems: Vec::new(), went_with }
}

/// What a clean-up removed that isn't in your Recycle Bin ([`in_the_bin`]'s other part): still left out of the sizes.
pub fn removed_for_good(done: &Done) -> Vec<String> {
    let not_found = |path: &str| done.problems.iter().any(|p| p.path == path);
    done.removed.iter().filter(|r| not_found(&r.path)).map(|r| r.path.clone()).collect()
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
    let presets_can_go = lines.iter().any(|l| l.stays.is_none() && l.folder.kind == SpaceKind::Presets);
    if inputs.ask.songs > 0 && presets_can_go {
        s.push_str(&format!(
            " {} {}. Its presets are safe to remove: songs keep their own settings.",
            if inputs.ask.songs == 1 { "1 song uses".to_owned() } else { format!("{} songs use", inputs.ask.songs) },
            inputs.ask.name
        ));
    }
    s
}

/// What a clean-up did: "Sent 4.2 GB: 2 folders are in the Recycle Bin, so you can put them back.", and each problem.
/// Not "freed": the Recycle Bin is on the same drive, so the room comes back once you empty it (as on Space).
pub fn done_text(done: &Done) -> (String, Vec<String>) {
    let went = done.removed.len();
    // One Windows removed for good (too big for the bin, and you said yes) isn't there to put back.
    let lost =
        done.removed.iter().filter(|r| done.problems.iter().any(|p| p.path == r.path && p.why == NOT_IN_BIN)).count();
    let binned = went - lost;
    let head = if went == 0 {
        "Nothing was removed.".to_owned()
    } else if binned == 0 {
        format!("Freed {}. None of it is in the Recycle Bin: the lines below say why.", size_text(done.bytes()))
    } else {
        // What went but Windows removed for good counts in what was sent, as on Space, and says so.
        let lost = match lost {
            0 => String::new(),
            1 => " 1 went but wasn't found there: the lines below say why.".to_owned(),
            n => format!(" {n} went but weren't found there: the lines below say why."),
        };
        format!(
            "Sent {}: {} in the Recycle Bin, so you can put {} back.{lost}",
            size_text(done.bytes()),
            {
                let (one, many) = things(done.removed.iter());
                if binned == 1 { format!("1 {one} is") } else { format!("{binned} {many} are") }
            },
            if binned == 1 { "it" } else { "them" }
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
            {
                let (one, many) = things(done.removed.iter());
                format!("Put back {}.", plural(back, one, many))
            }
        },
        problems.iter().map(|p| format!("{}: {}", p.path, p.why)).collect(),
    )
}

/// What still waits in the Recycle Bin after putting `done` back said `problems`: the folders that didn't come back,
/// or `None` when all of them did.
pub fn still_binned(done: &Done, problems: &[Problem]) -> Option<Done> {
    let removed: Vec<Removed> =
        done.removed.iter().filter(|r| problems.iter().any(|p| p.path == r.path)).cloned().collect();
    if removed.is_empty() {
        return None;
    }
    let went_with =
        done.went_with.iter().filter(|(_, outer)| removed.iter().any(|r| &r.path == outer)).cloned().collect();
    Some(Done { removed, problems: Vec::new(), went_with })
}

/// Several clean-ups as one, to put back together.
pub fn joined(dones: Vec<Done>) -> Done {
    dones.into_iter().fold(Done::default(), |mut all, d| {
        all.removed.extend(d.removed);
        all.problems.extend(d.problems);
        all.went_with.extend(d.went_with);
        all
    })
}

/// The line on the On disk tab for what this session's clean-ups of a plugin sent to the Recycle Bin, or "" for none.
pub fn binned_text(dones: &[&Done]) -> String {
    let n: usize = dones.iter().map(|d| d.removed.len()).sum();
    if n == 0 {
        return String::new();
    }
    let bytes = dones.iter().fold(0u64, |a, d| a.saturating_add(d.bytes()));
    let (one, many) = things(dones.iter().flat_map(|d| d.removed.iter()));
    let who = if one == "plugin file" { "Uninstall" } else { "Clean up" };
    format!("{who} sent {} to the Recycle Bin ({}).", plural(n, one, many), size_text(bytes))
}

/// What the things removed are called: plugin files when all of them are (Uninstall's), folders otherwise.
fn things<'a>(mut removed: impl Iterator<Item = &'a Removed>) -> (&'static str, &'static str) {
    if removed.all(|r| r.what == What::Plugin(SpaceKind::Program)) {
        ("plugin file", "plugin files")
    } else {
        ("folder", "folders")
    }
}

/// What the card says when the check couldn't be made.
pub fn check_failed_text(name: &str, why: &str) -> String {
    format!("Couldn't check {name}'s folders: {} Nothing was removed.", sentence(why))
}

/// What the card says when the folders couldn't be checked again just before they'd go.
pub fn recheck_failed_text(name: &str, why: &str) -> String {
    format!("Couldn't check {name}'s folders again before they went: {} Nothing was removed.", sentence(why))
}

/// What the card says when Clean up stopped part way on a bug.
pub const STOPPED_PART_WAY: &str =
    "Clean up stopped part way on something it didn't expect (the log says what). What went is in the Recycle Bin.";

/// What the card says when Put back stopped on a bug.
pub const PUT_BACK_STOPPED: &str = "Put back stopped on something it didn't expect (the log says what). What didn't come back is still in the \
     Recycle Bin.";

/// `why` ending in a full stop.
fn sentence(why: &str) -> String {
    let why = why.trim();
    if why.ends_with(['.', '!', '?']) { why.to_owned() } else { format!("{why}.") }
}

/// A bin that notes what it sent, so a clean-up that stopped on a bug part way still knows what went
/// ([`Watched::went`]). What went in Windows' administrator step goes another way and isn't noted: after such a stop
/// that is only found in the Recycle Bin by hand.
/// Each of [`Bin`]'s ways is passed on to `bin` (as `mo_space::clean::Heard` does, and a test checks each one): a way
/// added to [`Bin`] later must be passed on here too, or its default would stand in for `bin`'s own.
pub struct Watched<'a> {
    bin: &'a (dyn Bin + Sync),
    sent: Mutex<Vec<(String, i64)>>,
}

impl<'a> Watched<'a> {
    pub fn new(bin: &'a (dyn Bin + Sync)) -> Self {
        Watched { bin, sent: Mutex::new(Vec::new()) }
    }

    /// What went to the bin, as the lines picked had it, found in the bin so it can be put back. For a clean-up that
    /// stopped part way: one that finished says this itself.
    pub fn went(&self, picked: &[CleanLine]) -> Done {
        let lines: Vec<(&str, u64, SpaceKind)> =
            picked.iter().map(|l| (l.folder.path.as_str(), l.bytes(), l.folder.kind)).collect();
        self.went_of(&lines)
    }

    /// [`Watched::went`] for any lines picked: each one's path, the room it takes, and what it is.
    pub fn went_of(&self, picked: &[(&str, u64, SpaceKind)]) -> Done {
        let sent = self.sent.lock().map(|s| s.clone()).unwrap_or_else(|e| e.into_inner().clone());
        let mut removed: Vec<Removed> = sent
            .into_iter()
            .map(|(real, when)| {
                let line = picked.iter().find(|l| key(l.0) == key(&real)).or_else(|| {
                    let real = std::fs::canonicalize(Path::new(&real)).ok();
                    picked.iter().find(|l| std::fs::canonicalize(l.0).ok() == real && real.is_some())
                });
                Removed {
                    path: line.map_or_else(|| real.clone(), |l| l.0.to_owned()),
                    bytes: line.map_or(0, |l| l.1),
                    what: What::Plugin(line.map_or(SpaceKind::Sounds, |l| l.2)),
                    real,
                    when,
                    bin_id: None,
                    binned: None,
                    binned_zone: None,
                    stamp: None,
                }
            })
            .collect();
        let problems = self.bin.find(&mut removed);
        Done { removed, problems, went_with: Vec::new() }
    }
}

impl Bin for Watched<'_> {
    fn send(&self, path: &Path) -> Result<(), String> {
        let when = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        self.bin.send(path)?;
        let mut sent = self.sent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        sent.push((path.to_string_lossy().into_owned(), when));
        Ok(())
    }
    fn admin(&self) -> &dyn Admin {
        self.bin.admin()
    }
    fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
        self.bin.find(removed)
    }
    fn put_back(&self, removed: &[Removed]) -> Vec<Problem> {
        self.bin.put_back(removed)
    }
    fn holds(&self, removed: &[Removed]) -> Option<Vec<bool>> {
        self.bin.holds(removed)
    }
    fn put_back_checked(
        &self,
        removed: &[Removed],
        check: &mo_space::clean::Check<'_>,
        each: &mut dyn FnMut(usize, Option<String>),
    ) {
        self.bin.put_back_checked(removed, check, each);
    }
    fn can_look_for(&self, removed: &Removed) -> bool {
        self.bin.can_look_for(removed)
    }
    fn refuses_quietly(&self, path: &Path, bytes: u64) -> Option<String> {
        self.bin.refuses_quietly(path, bytes)
    }
    fn room(&self, real: &Path) -> Option<Result<Room, String>> {
        self.bin.room(real)
    }
    fn going(&self, removed: &Removed) {
        self.bin.going(removed);
    }
}
