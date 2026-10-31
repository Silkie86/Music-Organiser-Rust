//! Clean up: removing what you picked to free space, safely.
//!
//! It goes in three steps, so nothing is removed that you haven't seen and nothing is lost for good:
//!
//! 1. [`preview`] checks every picked line against the [`Guard`] and says what would go and what wouldn't (and why).
//!    Nothing on disk changes.
//! 2. [`apply`] checks each line again just before it goes (one that changed since it was measured stays) and sends it
//!    to the Recycle Bin (the Trash on a Mac or Linux). Nothing is ever deleted for good here. Then it finds each one
//!    in the bin, so it can be put back, and returns a [`Done`] list.
//! 3. [`undo`] puts the [`Done`] list back from the bin, where it was.
//!
//! The guard keeps the C# app's rules: samples in your sample folders and audio any project uses never go (rule 2),
//! every song's newest save stays, a save that's a copy only goes while the save it copies is still there and still
//! the same, and only things inside your project folders are touched at all. Paths are checked as written and as they
//! really are on disk, so a link can't lead a clean-up out of your project folders or into a sample folder.

use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::io::Read;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, NaiveDateTime};
use mo_formats::case;
use mo_formats::paths::file_name;
use mo_formats::textscan::is_audio_path;
use mo_library::scan::{daw_of, is_autosave_dir, is_autosave_file, is_inside, is_project_file};
use serde::{Deserialize, Serialize};

use crate::plugin_space::SpaceKind;
use crate::song::{CompactItem, CompactKind, has_studio_one_song, parent};
use crate::{key, real_path};

/// Why a picked line won't go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Refused {
    /// It isn't there any more.
    Gone,
    /// It's a link to another place: removing it wouldn't free anything.
    Link,
    /// It's reached through a link that leads out of your project folder.
    ThroughLink,
    /// It isn't inside any of your project folders.
    OutsideProjects,
    /// It's one of your project folders, or holds one.
    ProjectFolder,
    /// It's on a network drive, which has no Recycle Bin.
    NetworkDrive,
    /// It's in one of your sample folders, or holds one.
    InSampleFolder,
    /// A project uses an audio file of this name.
    SampleInUse,
    /// It's a song's newest save, which always stays.
    KeptSave,
    /// The line says it's a save, but it isn't a project file.
    NotASave,
    /// The line says it's an autosave, but it isn't one.
    NotAnAutosave,
    /// The line says it's a cache, but it isn't Studio One's Cache folder.
    NotACache,
    /// The line says it's audio, but it isn't an audio file.
    NotAudio,
    /// The line says it's a copy, but the save it copies has gone or isn't the same any more.
    NotACopyNow,
    /// The line is a copy of a save that's going too: one of them has to stay.
    TwinGoesToo,
    /// It changed since it was measured.
    Changed,
}

impl Refused {
    /// What the app says about it.
    pub fn text(self) -> &'static str {
        match self {
            Refused::Gone => "It isn't there any more.",
            Refused::Link => "It's a link to another place, so removing it frees nothing.",
            Refused::ThroughLink => "It's reached through a link to another place, outside your project folder.",
            Refused::OutsideProjects => "It isn't in one of your project folders.",
            Refused::ProjectFolder => "It's one of your project folders.",
            Refused::NetworkDrive => "It's on a network drive, which has no Recycle Bin.",
            Refused::InSampleFolder => "It's in one of your sample folders, and samples are never removed.",
            Refused::SampleInUse => "A project uses a sound with this name.",
            Refused::KeptSave => "It's the song's newest save, which always stays.",
            Refused::NotASave => "It isn't a project save.",
            Refused::NotAnAutosave => "It isn't one of the DAW's autosaves.",
            Refused::NotACache => "It isn't Studio One's Cache folder.",
            Refused::NotAudio => "It isn't an audio file.",
            Refused::NotACopyNow => "The save it was a copy of has changed or gone, so it stays.",
            Refused::TwinGoesToo => "The save it's a copy of is going too, so this one stays.",
            Refused::Changed => "It changed since you looked, so it stays.",
        }
    }
}

/// What must never go. The app fills it from the library.
pub struct Guard<'a> {
    /// Your project folders: only things inside them are touched.
    pub project_roots: &'a [String],
    /// Your sample folders: nothing in them is touched.
    pub sample_folders: &'a [String],
    /// Saves that always stay, as [`key`]s: every song's newest save.
    pub keep: &'a HashSet<String>,
    /// Whether any project uses an audio file of this name (letter case aside).
    pub sample_in_use: &'a (dyn Fn(&str) -> bool + Sync),
}

/// What a line looked like when it was checked, to see at the end whether it changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    folder: bool,
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    fn of(meta: &Metadata) -> Stamp {
        Stamp { folder: meta.is_dir(), len: meta.len(), modified: meta.modified().ok() }
    }
}

/// A line the guard let through: what it looked like, and where it really is.
struct Checked {
    stamp: Stamp,
    real: String,
}

/// A folder as written, and where it really is (as written when that can't be read).
pub(crate) fn both(folder: &str) -> (&str, String) {
    (folder, real_path(folder).unwrap_or_else(|| folder.to_owned()))
}

/// Whether a path is on a network drive: written with a server's name (`\\server\share`, what a mapped drive letter
/// really is), or on a Mac or Linux, on a network share mounted into a folder (`/Volumes/Share`, `/mnt/nas`).
pub(crate) fn on_network(path: &str) -> bool {
    path.starts_with(r"\\") || path.starts_with("//") || network_mount(path)
}

/// A Mac says which drives are its own: any other is on the network.
#[cfg(target_os = "macos")]
fn network_mount(path: &str) -> bool {
    const MNT_LOCAL: u32 = 0x1000;
    rustix::fs::statfs(path).is_ok_and(|s| s.f_flags & MNT_LOCAL == 0)
}

/// Linux says what kind of drive a folder is on: NFS, Windows shares (SMB, CIFS), AFS, Coda, NetWare, 9P.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn network_mount(path: &str) -> bool {
    use rustix::fs::FsWord;
    const NETWORK: [u32; 8] = [0x6969, 0x517B, 0xFF53_4D42, 0xFE53_4D42, 0x5346_414F, 0x7375_7245, 0x564C, 0x0102_1997];
    rustix::fs::statfs(path).is_ok_and(|s| NETWORK.iter().any(|&m| s.f_type == m as FsWord))
}

/// Elsewhere only the path tells.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
fn network_mount(_path: &str) -> bool {
    false
}

impl Guard<'_> {
    /// Whether a line may go, and what it looks like now.
    fn check(&self, item: &CompactItem) -> Result<Checked, Refused> {
        let path = item.path.as_str();
        let meta = fs::symlink_metadata(path).map_err(|_| Refused::Gone)?;
        if meta.file_type().is_symlink() {
            return Err(Refused::Link);
        }
        // Where it really is: a mapped network drive reads as a server's name here.
        let real = real_path(path).ok_or(Refused::Gone)?;
        if on_network(path) || on_network(&real) {
            return Err(Refused::NetworkDrive);
        }
        let roots: Vec<(&str, String)> = self.project_roots.iter().map(|r| both(r)).collect();
        if roots.iter().any(|(r, real_r)| is_inside(r, path) || is_inside(real_r, &real)) {
            return Err(Refused::ProjectFolder);
        }
        // The innermost project folder it's in, to read the folders below it.
        let Some((root, real_root)) =
            roots.iter().filter(|(r, _)| is_inside(path, r)).max_by_key(|(r, _)| parts(r).count())
        else {
            return Err(Refused::OutsideProjects);
        };
        if !is_inside(&real, real_root) {
            return Err(Refused::ThroughLink);
        }
        let in_samples = self.sample_folders.iter().map(|f| both(f)).any(|(f, real_f)| {
            is_inside(path, f) || is_inside(f, path) || is_inside(&real, &real_f) || is_inside(&real_f, &real)
        });
        if in_samples {
            return Err(Refused::InSampleFolder);
        }
        if self.keep.contains(&key(path)) || self.keep.contains(&key(&real)) {
            return Err(Refused::KeptSave);
        }
        let name = file_name(path);
        match item.kind {
            CompactKind::SameSave | CompactKind::ThinnedSave => {
                if !meta.is_file() || !is_project_file(path) {
                    return Err(Refused::NotASave);
                }
            }
            CompactKind::Backup => {
                // Only the folders below the project folder count: a project folder called "Backup" doesn't make every
                // song in it an autosave.
                let all: Vec<&str> = parts(path).collect();
                let below_root = all.get(parts(root).count()..all.len().saturating_sub(1)).unwrap_or_default();
                let autosave = is_autosave_file(name) || below_root.iter().any(|d| is_autosave_dir(d));
                if !meta.is_file() || daw_of(path).is_none() || !autosave {
                    return Err(Refused::NotAnAutosave);
                }
            }
            CompactKind::Cache => {
                if !meta.is_dir()
                    || !case::eq_ignore_case(name, "Cache")
                    || !has_studio_one_song(Path::new(&parent(path)))
                {
                    return Err(Refused::NotACache);
                }
            }
            CompactKind::UnusedAudio => {
                if !meta.is_file() || !is_audio_path(path) {
                    return Err(Refused::NotAudio);
                }
                if (self.sample_in_use)(name) {
                    return Err(Refused::SampleInUse);
                }
            }
        }
        // A file that isn't as it was measured (written again since) stays. A folder's size is what it holds, so only
        // a file's is compared.
        if meta.is_file() && (meta.len() != item.bytes || !same_time(&meta, item.modified)) {
            return Err(Refused::Changed);
        }
        if item.kind == CompactKind::SameSave && !still_same(item, &real) {
            return Err(Refused::NotACopyNow);
        }
        Ok(Checked { stamp: Stamp::of(&meta), real })
    }
}

/// Whether a file was last written when the line says (to the second, either way), or the line doesn't say.
fn same_time(meta: &Metadata, measured: Option<NaiveDateTime>) -> bool {
    let Some(measured) = measured else { return true };
    let Ok(modified) = meta.modified() else { return false };
    (DateTime::<Local>::from(modified).naive_local() - measured).num_milliseconds().abs() < 2000
}

/// The names in a path, whichever way its slashes lean.
pub(crate) fn parts(path: &str) -> impl Iterator<Item = &str> {
    path.split(['\\', '/']).filter(|d| !d.is_empty())
}

/// Whether a copy's twin is still there, a file of its own (not a link, not the copy itself), and still byte for byte
/// the same.
fn still_same(item: &CompactItem, real: &str) -> bool {
    let Some(twin) = item.same_as.as_deref() else { return false };
    let twin_is_a_file = fs::symlink_metadata(twin).is_ok_and(|m| m.is_file());
    let Some(real_twin) = real_path(twin) else { return false };
    if !twin_is_a_file || key(&real_twin) == key(real) {
        return false;
    }
    same_bytes(Path::new(&item.path), Path::new(twin)).unwrap_or(false)
}

/// Whether two files hold the same bytes, read side by side in pieces.
fn same_bytes(a: &Path, b: &Path) -> std::io::Result<bool> {
    let (mut fa, mut fb) = (fs::File::open(a)?, fs::File::open(b)?);
    if fa.metadata()?.len() != fb.metadata()?.len() {
        return Ok(false);
    }
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let n = fa.read(&mut ba)?;
        if n == 0 {
            // The first has ended: the same only if the second has too.
            return Ok(fb.read(&mut bb)? == 0);
        }
        let (Some(want), Some(got)) = (ba.get(..n), bb.get_mut(..n)) else { return Ok(false) };
        if fb.read_exact(got).is_err() || want != &*got {
            return Ok(false);
        }
    }
}

/// A line that will go, and what it looked like when checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanLine {
    pub item: CompactItem,
    stamp: Stamp,
}

/// What a clean-up would do: the lines that would go, and the ones that wouldn't, with why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub lines: Vec<PlanLine>,
    pub refused: Vec<(CompactItem, Refused)>,
}

impl Plan {
    /// How much room the lines that would go take.
    pub fn bytes(&self) -> u64 {
        self.lines.iter().map(|l| l.item.bytes).sum()
    }
}

/// Checks what you picked and says what would go. Nothing on disk changes. A line picked twice counts once, and a line
/// inside a folder that's going anyway goes with it.
pub fn preview(picked: &[CompactItem], guard: &Guard<'_>) -> Plan {
    let mut plan = Plan::default();
    let mut seen: HashSet<String> = HashSet::new();
    for item in picked {
        if !seen.insert(key(&item.path)) {
            continue;
        }
        match guard.check(item) {
            Ok(checked) => plan.lines.push(PlanLine { item: item.clone(), stamp: checked.stamp }),
            Err(why) => plan.refused.push((item.clone(), why)),
        }
    }
    // A copy whose twin goes too would lose what they both held: the copy stays.
    let going: HashSet<String> = plan.lines.iter().map(|l| key(&l.item.path)).collect();
    let (twins, lines): (Vec<PlanLine>, Vec<PlanLine>) = plan.lines.into_iter().partition(|l| {
        l.item.kind == CompactKind::SameSave && going.contains(&key(l.item.same_as.as_deref().unwrap_or_default()))
    });
    plan.lines = lines;
    plan.refused.extend(twins.into_iter().map(|l| (l.item, Refused::TwinGoesToo)));
    let folders: Vec<String> = plan.lines.iter().filter(|l| l.stamp.folder).map(|l| l.item.path.clone()).collect();
    plan.lines.retain(|l| !folders.iter().any(|f| key(f) != key(&l.item.path) && is_inside(&l.item.path, f)));
    plan
}

/// Something that went to the Recycle Bin: enough to put it back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Removed {
    /// Where it was, as the line had it.
    pub path: String,
    /// Where it really was (links worked out): what the bin keeps.
    pub real: String,
    pub bytes: u64,
    pub what: What,
    /// When it went (Unix seconds, taken just before).
    pub when: i64,
    /// What the bin calls it, once found there.
    pub bin_id: Option<String>,
}

/// What something removed was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum What {
    /// A line of a song's Compact list.
    Song(CompactKind),
    /// A plugin's presets or sounds folder.
    Plugin(SpaceKind),
}

/// A line that couldn't go (or come back), and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub path: String,
    pub why: String,
}

/// What a clean-up did: kept by the app so it can be undone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    pub removed: Vec<Removed>,
    pub problems: Vec<Problem>,
}

impl Done {
    /// How much room was freed.
    pub fn bytes(&self) -> u64 {
        self.removed.iter().map(|r| r.bytes).sum()
    }
}

/// Where removed things go. The app uses [`SystemBin`]; tests use a folder of their own.
pub trait Bin {
    /// Puts a file or folder in the bin, or says what stopped it.
    fn send(&self, path: &Path) -> Result<(), String>;
    /// After a clean-up: finds each removed thing in the bin and notes what the bin calls it, so it can be put back.
    /// Says which aren't there (removed for good when Windows asked about something too big for the bin).
    fn find(&self, removed: &mut [Removed]) -> Vec<Problem>;
    /// Puts things back from the bin where they were; says what couldn't come back.
    fn put_back(&self, removed: &[Removed]) -> Vec<Problem>;
}

/// Sends each line of the plan to the bin, checking each again just before: one that changed, or that the guard now
/// refuses, stays. A copy only goes while the save it copies is still there and still the same.
pub fn apply(plan: &Plan, guard: &Guard<'_>, bin: &dyn Bin) -> Done {
    let mut done = Done::default();
    for line in &plan.lines {
        let item = &line.item;
        let real = match guard.check(item) {
            Err(why) => Err(why),
            Ok(now) if now.stamp != line.stamp => Err(Refused::Changed),
            Ok(now) => Ok(now.real),
        };
        let real = match real {
            Ok(real) => real,
            Err(why) => {
                done.problems.push(Problem { path: item.path.clone(), why: why.text().to_owned() });
                continue;
            }
        };
        send(&item.path, real, item.bytes, What::Song(item.kind), bin, &mut done);
    }
    let not_found = bin.find(&mut done.removed);
    done.problems.extend(not_found);
    done
}

/// Sends one checked thing to the bin and notes how it went.
pub(crate) fn send(path: &str, real: String, bytes: u64, what: What, bin: &dyn Bin, done: &mut Done) {
    let when = now_seconds();
    match bin.send(Path::new(path)) {
        Ok(()) => done.removed.push(Removed { path: path.to_owned(), real, bytes, what, when, bin_id: None }),
        Err(why) => done.problems.push(Problem { path: path.to_owned(), why }),
    }
}

/// Puts back what a clean-up removed.
pub fn undo(done: &Done, bin: &dyn Bin) -> Vec<Problem> {
    bin.put_back(&done.removed)
}

fn now_seconds() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The computer's own Recycle Bin (the Trash on a Mac or Linux).
pub struct SystemBin;

impl Bin for SystemBin {
    fn send(&self, path: &Path) -> Result<(), String> {
        // The library behind this can stop the thread when Windows won't start its file operations; that's a problem
        // for this line, not a crash.
        let sent = catch_unwind(AssertUnwindSafe(|| trash::delete(path)))
            .map_err(|_| "Windows wouldn't start moving it to the Recycle Bin.".to_owned())?;
        match sent {
            Ok(()) if fs::symlink_metadata(path).is_ok() => {
                Err("It's still there: you chose to keep it, or a program is using some of it.".to_owned())
            }
            Ok(()) => Ok(()),
            Err(e) => Err(bin_problem(&e)),
        }
    }

    fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
        find_in_system_bin(removed)
    }

    fn put_back(&self, removed: &[Removed]) -> Vec<Problem> {
        put_back_from_system_bin(removed)
    }
}

fn bin_problem(e: &trash::Error) -> String {
    match e {
        // HRESULTs from Windows: access denied, file in use.
        trash::Error::Os { code, .. } if code & 0xFFFF == 5 || code & 0xFFFF == 0x78 => {
            "Windows needs administrator rights to remove this; remove it in Explorer instead.".to_owned()
        }
        trash::Error::Os { code, .. } if code & 0xFFFF == 32 => {
            "A program is using it. Close your DAW and try again.".to_owned()
        }
        trash::Error::Unknown { description } if description.contains("aborted") => "You chose to keep it.".to_owned(),
        trash::Error::CouldNotAccess { .. } => "It couldn't be reached.".to_owned(),
        _ => format!("It couldn't go to the Recycle Bin ({e:?})."),
    }
}

/// Whether a bin item is the one removed: from the same place, and gone to the bin at or after the moment it was sent
/// (a couple of seconds' leeway for the clock).
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn is_the_one(item: &trash::TrashItem, r: &Removed) -> bool {
    key(&item.original_path().to_string_lossy()) == key(&r.real) && item.time_deleted >= r.when - 2
}

#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn bin_items() -> Option<Vec<trash::TrashItem>> {
    match catch_unwind(trash::os_limited::list) {
        Ok(Ok(items)) => Some(items),
        _ => None,
    }
}

#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn find_in_system_bin(removed: &mut [Removed]) -> Vec<Problem> {
    let Some(items) = bin_items() else {
        return removed
            .iter()
            .map(|r| Problem {
                path: r.path.clone(),
                why: "The Recycle Bin couldn't be read to check it's there.".into(),
            })
            .collect();
    };
    let mut taken: HashSet<&std::ffi::OsStr> = HashSet::new();
    let mut problems = Vec::new();
    for r in removed.iter_mut() {
        // The first to go there after it was sent is this one.
        let found = items
            .iter()
            .filter(|i| !taken.contains(i.id.as_os_str()) && is_the_one(i, r))
            .min_by_key(|i| i.time_deleted);
        match found {
            Some(item) => {
                taken.insert(item.id.as_os_str());
                r.bin_id = item.id.to_str().map(str::to_owned);
            }
            None => problems.push(Problem {
                path: r.path.clone(),
                why: "It isn't in the Recycle Bin, so it may have been removed for good.".into(),
            }),
        }
    }
    problems
}

#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn put_back_from_system_bin(removed: &[Removed]) -> Vec<Problem> {
    let problem = |r: &Removed, why: &str| Problem { path: r.path.clone(), why: why.to_owned() };
    let Some(items) = bin_items() else {
        return removed.iter().map(|r| problem(r, "The Recycle Bin couldn't be read.")).collect();
    };
    let mut problems = Vec::new();
    for r in removed {
        let by_id = r.bin_id.as_deref().and_then(|id| items.iter().find(|i| i.id.to_str() == Some(id)));
        let item = by_id.or_else(|| items.iter().filter(|i| is_the_one(i, r)).min_by_key(|i| i.time_deleted));
        let Some(item) = item else {
            problems.push(problem(r, "It isn't in the Recycle Bin any more."));
            continue;
        };
        match catch_unwind(AssertUnwindSafe(|| trash::os_limited::restore_all([item.clone()]))) {
            Ok(Ok(())) => {}
            Ok(Err(trash::Error::RestoreCollision { .. })) => {
                problems.push(problem(r, "Something new has its name now, so it's left in the Recycle Bin."))
            }
            _ => problems.push(problem(r, "It couldn't be put back; it's still in the Recycle Bin.")),
        }
    }
    problems
}

/// A Mac's Trash can't be read by a program: nothing to check.
#[cfg(not(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
fn find_in_system_bin(_removed: &mut [Removed]) -> Vec<Problem> {
    Vec::new()
}

/// A Mac's Trash can't be asked to put things back by a program: Finder's Put Back does it.
#[cfg(not(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
fn put_back_from_system_bin(removed: &[Removed]) -> Vec<Problem> {
    removed
        .iter()
        .map(|r| Problem {
            path: r.path.clone(),
            why: "Put it back from the Trash: select it there and choose Put Back.".to_owned(),
        })
        .collect()
}
