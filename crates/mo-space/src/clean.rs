//! Clean up: removing what you picked to free space, safely.
//!
//! It goes in three steps, so nothing is removed that you haven't seen and nothing is lost for good:
//!
//! 1. [`preview`] checks every picked line against the [`Guard`] and says what would go and what wouldn't (and why).
//!    Nothing on disk changes.
//! 2. [`apply`] checks each line again (a file changed since the preview stays) and sends it to the Recycle Bin (the
//!    Trash on a Mac or Linux). Nothing is ever deleted for good here. It returns a [`Done`] list.
//! 3. [`undo`] puts the [`Done`] list back from the bin, where it was.
//!
//! The guard keeps the C# app's rules: samples in your sample folders and audio any project uses never go (rule 2),
//! every song's newest save stays, a save that's a copy only goes while the save it copies is still there and still
//! the same, and only things inside your project folders are touched at all.

use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use mo_formats::case;
use mo_formats::paths::{extension, file_name};
use mo_formats::textscan::is_audio_path;
use mo_library::scan::{daw_of, is_autosave_dir, is_autosave_file, is_inside, is_project_file};
use serde::{Deserialize, Serialize};

use crate::key;
use crate::song::{CompactItem, CompactKind, parent};

/// Why a picked line won't go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Refused {
    /// It isn't there any more.
    Gone,
    /// It's a link to another place: removing it wouldn't free anything.
    Link,
    /// It isn't inside any of your project folders.
    OutsideProjects,
    /// It's one of your project folders, or holds one.
    ProjectFolder,
    /// It's on a network drive, which has no Recycle Bin.
    NetworkDrive,
    /// It's in one of your sample folders.
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
    /// It changed since the preview.
    Changed,
}

impl Refused {
    /// What the app says about it.
    pub fn text(self) -> &'static str {
        match self {
            Refused::Gone => "It isn't there any more.",
            Refused::Link => "It's a link to another place, so removing it frees nothing.",
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

impl Guard<'_> {
    /// Whether a line may go, and what it looks like now.
    fn check(&self, item: &CompactItem) -> Result<Stamp, Refused> {
        let path = item.path.as_str();
        let meta = fs::symlink_metadata(path).map_err(|_| Refused::Gone)?;
        if meta.file_type().is_symlink() {
            return Err(Refused::Link);
        }
        if path.starts_with(r"\\") || path.starts_with("//") {
            return Err(Refused::NetworkDrive);
        }
        if self.project_roots.iter().any(|r| is_inside(r, path)) {
            return Err(Refused::ProjectFolder);
        }
        // The innermost project folder it's in, to read the folders below it.
        let Some(root) = self.project_roots.iter().filter(|r| is_inside(path, r)).max_by_key(|r| parts(r).count())
        else {
            return Err(Refused::OutsideProjects);
        };
        if self.sample_folders.iter().any(|f| is_inside(path, f) || is_inside(f, path)) {
            return Err(Refused::InSampleFolder);
        }
        if self.keep.contains(&key(path)) {
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
                let all: Vec<&str> = parts(path).collect();
                let below_root = all.get(parts(root).count()..all.len().saturating_sub(1)).unwrap_or_default();
                let autosave = is_autosave_file(name) || below_root.iter().any(|d| is_autosave_dir(d));
                if !meta.is_file() || daw_of(path).is_none() || !autosave {
                    return Err(Refused::NotAnAutosave);
                }
            }
            CompactKind::Cache => {
                if !meta.is_dir() || !case::eq_ignore_case(name, "Cache") || !beside_a_studio_one_song(path) {
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
        Ok(Stamp::of(&meta))
    }
}

/// The names in a path, whichever way its slashes lean.
fn parts(path: &str) -> impl Iterator<Item = &str> {
    path.split(['\\', '/']).filter(|d| !d.is_empty())
}

/// Whether the folder beside `cache` holds a Studio One song.
fn beside_a_studio_one_song(cache: &str) -> bool {
    fs::read_dir(parent(cache)).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            e.file_type().is_ok_and(|t| t.is_file())
                && case::eq_ignore_case(extension(&e.file_name().to_string_lossy()), ".song")
        })
    })
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
            Ok(stamp) => plan.lines.push(PlanLine { item: item.clone(), stamp }),
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
    pub path: String,
    pub bytes: u64,
    pub kind: CompactKind,
    /// When it went (Unix seconds), so the right one is put back when the bin holds several of the same name.
    pub when: i64,
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
    /// Puts things back from the bin where they were; says what couldn't come back.
    fn put_back(&self, removed: &[Removed]) -> Vec<Problem>;
}

/// Sends each line of the plan to the bin, checking each again first: one that changed since the preview, or that the
/// guard now refuses, stays. A copy only goes while the save it copies is still there and still the same.
pub fn apply(plan: &Plan, guard: &Guard<'_>, bin: &dyn Bin) -> Done {
    let mut done = Done::default();
    for line in &plan.lines {
        let item = &line.item;
        let stays = match guard.check(item) {
            Err(why) => Some(why),
            Ok(now) if now != line.stamp => Some(Refused::Changed),
            Ok(_) if item.kind == CompactKind::SameSave && !still_same(item) => Some(Refused::NotACopyNow),
            Ok(_) => None,
        };
        if let Some(why) = stays {
            done.problems.push(Problem { path: item.path.clone(), why: why.text().to_owned() });
            continue;
        }
        let when = now_seconds();
        match bin.send(Path::new(&item.path)) {
            Ok(()) => done.removed.push(Removed { path: item.path.clone(), bytes: item.bytes, kind: item.kind, when }),
            Err(why) => done.problems.push(Problem { path: item.path.clone(), why }),
        }
    }
    done
}

/// Puts back what a clean-up removed.
pub fn undo(done: &Done, bin: &dyn Bin) -> Vec<Problem> {
    bin.put_back(&done.removed)
}

/// Whether a copy's twin is still there and still byte for byte the same.
fn still_same(item: &CompactItem) -> bool {
    let Some(twin) = item.same_as.as_deref() else { return false };
    if key(twin) == key(&item.path) {
        return false;
    }
    same_bytes(Path::new(&item.path), Path::new(twin)).unwrap_or(false)
}

/// Whether two files hold the same bytes, read side by side in pieces.
fn same_bytes(a: &Path, b: &Path) -> std::io::Result<bool> {
    use std::io::Read;
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

#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn put_back_from_system_bin(removed: &[Removed]) -> Vec<Problem> {
    use trash::os_limited::{list, restore_all};

    let problem = |r: &Removed, why: &str| Problem { path: r.path.clone(), why: why.to_owned() };
    let in_bin = match catch_unwind(list) {
        Ok(Ok(items)) => items,
        _ => return removed.iter().map(|r| problem(r, "The Recycle Bin couldn't be read.")).collect(),
    };
    let mut problems = Vec::new();
    let mut used: HashSet<std::ffi::OsString> = HashSet::new();
    for r in removed {
        let at = bin_time(r.when);
        // The one of this name that went then (a minute either way); of several, the latest.
        let item = in_bin
            .iter()
            .filter(|i| !used.contains(&i.id))
            .filter(|i| key(&i.original_path().to_string_lossy()) == key(&r.path))
            .filter(|i| (i.time_deleted - at).abs() <= 60)
            .max_by_key(|i| i.time_deleted);
        let Some(item) = item else {
            problems.push(problem(r, "It isn't in the Recycle Bin any more."));
            continue;
        };
        used.insert(item.id.clone());
        match catch_unwind(AssertUnwindSafe(|| restore_all([item.clone()]))) {
            Ok(Ok(())) => {}
            Ok(Err(trash::Error::RestoreCollision { .. })) => {
                problems.push(problem(r, "Something new has its name now, so it's left in the Recycle Bin."))
            }
            _ => problems.push(problem(r, "It couldn't be put back; it's still in the Recycle Bin.")),
        }
    }
    problems
}

/// When the bin says something went: Windows says it in Unix seconds; the Linux bin writes local time, which the
/// library reads as if it were UTC.
#[cfg(windows)]
fn bin_time(when: i64) -> i64 {
    when
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
fn bin_time(when: i64) -> i64 {
    use chrono::{Local, TimeZone};
    let offset = Local.timestamp_opt(when, 0).single().map_or(0, |t| t.offset().local_minus_utc());
    when + i64::from(offset)
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
