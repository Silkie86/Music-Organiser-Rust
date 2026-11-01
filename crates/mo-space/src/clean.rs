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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::NaiveDateTime;
use mo_formats::case;
use mo_formats::paths::file_name;
use mo_formats::textscan::is_audio_path;
use mo_library::scan::{daw_of, is_autosave_dir, is_autosave_file, is_inside, is_project_file};
use serde::{Deserialize, Serialize};

use crate::admin::{Admin, AsAdmin, NoAdmin, Step, ask_for, real_spot, reason, stamp};
use crate::plugin_space::SpaceKind;
use crate::size::{OnlineOnly, Stopped, is_link, is_online_only, modified_of, online_only_in, total};
use crate::song::{CompactItem, CompactKind, has_studio_one_song, parent};
use crate::{key, not_there, real_path, text};

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
    /// You pressed Stop before it went.
    Stopped,
    /// It's on a USB stick or memory card, which has no Recycle Bin (Windows).
    RemovableDrive,
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
            Refused::Stopped => STOPPED,
            Refused::RemovableDrive => {
                "It's on a USB stick or memory card, which has no Recycle Bin: removing it would delete it for good."
            }
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

/// Linux says what kind of drive a folder is on: NFS, Windows shares (SMB, CIFS), AFS, Coda, NetWare, 9P, Ceph. A FUSE
/// drive (sshfs, rclone, the Files app's shares) counts as network unless it's a disk (`fuseblk`: NTFS, exFAT).
#[cfg(any(target_os = "linux", target_os = "android"))]
fn network_mount(path: &str) -> bool {
    use rustix::fs::FsWord;
    const NETWORK: [u32; 10] = [
        0x6969,
        0x517B,
        0xFF53_4D42,
        0xFE53_4D42,
        0x5346_414F,
        0x6B41_4653,
        0x7375_7245,
        0x564C,
        0x0102_1997,
        0x00C3_6400,
    ];
    const FUSE: u32 = 0x6573_5546;
    let Ok(s) = rustix::fs::statfs(path) else { return false };
    NETWORK.iter().any(|&m| s.f_type == m as FsWord) || (s.f_type == FUSE as FsWord && !fuse_disk(path))
}

/// Whether the FUSE drive a folder is on is a disk, from the system's list of drives (the innermost one holding it).
#[cfg(any(target_os = "linux", target_os = "android"))]
fn fuse_disk(path: &str) -> bool {
    let Ok(table) = fs::read_to_string("/proc/self/mounts") else { return false };
    let real = real_path(path).unwrap_or_else(|| path.to_owned());
    table
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            let _device = fields.next()?;
            // Spaces in a folder's name are written \040.
            let at = fields.next()?.replace("\\040", " ");
            Some((at, fields.next()?.to_owned()))
        })
        .filter(|(at, _)| is_inside(&real, at))
        .max_by_key(|(at, _)| at.len())
        .is_some_and(|(_, kind)| kind == "fuseblk")
}

/// Elsewhere only the path tells.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
fn network_mount(_path: &str) -> bool {
    false
}

/// Whether a file or folder (where it really is) is on a USB stick or memory card. Windows keeps no Recycle Bin on
/// one, so removing from it deletes for good. (A Mac and Linux keep a Trash on each drive.) The list of drives is read
/// at most every few seconds, as one look at hundreds of plugins asks about each.
#[cfg(windows)]
pub(crate) fn on_removable(real: &str) -> bool {
    use std::sync::{Mutex, PoisonError};
    use std::time::{Duration, Instant};
    /// The drives as last read: where each is, and whether it's removable.
    type Drives = Vec<(String, bool)>;
    static READ: Mutex<Option<(Instant, Drives)>> = Mutex::new(None);
    let mut read = READ.lock().unwrap_or_else(PoisonError::into_inner);
    let (_, drives) = match read.take() {
        Some((at, drives)) if at.elapsed() < Duration::from_secs(5) => read.insert((at, drives)),
        _ => {
            // A list that can't be read says nothing is removable: as it was before this check.
            let disks = sysinfo::Disks::new_with_refreshed_list_specifics(sysinfo::DiskRefreshKind::nothing());
            let drives = disks
                .map_or_else(|_| Vec::new(), |d| d.iter().map(|d| (text(d.mount_point()), d.is_removable())).collect());
            read.insert((Instant::now(), drives))
        }
    };
    // The innermost drive holding it decides: a drive can sit in a folder of another.
    drives.iter().filter(|(at, _)| is_inside(real, at)).max_by_key(|(at, _)| at.len()).is_some_and(|(_, r)| *r)
}

/// A Mac and Linux keep a Trash on each drive, a USB stick too.
#[cfg(not(windows))]
pub(crate) fn on_removable(_real: &str) -> bool {
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
        if on_removable(&real) {
            return Err(Refused::RemovableDrive);
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

/// Whether a file was last written when the line says (to the second, either way), or the line doesn't say. A time
/// that can't be told is read as the measure read it (the start of 1970), so such a file isn't always taken as changed.
fn same_time(meta: &Metadata, measured: Option<NaiveDateTime>) -> bool {
    let Some(measured) = measured else { return true };
    (modified_of(meta) - measured).num_milliseconds().abs() < 2000
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
        total(self.lines.iter().map(|l| l.item.bytes))
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
    /// Something of a plugin's: its presets or sounds folder, or a plugin file or bundle (a spare copy).
    Plugin(SpaceKind),
    /// A Bitwig project's copy of a Library multisample, once a "(fixed)" project points at the Library file.
    BitwigCopy,
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
    /// Picked folders that went inside another picked folder (where it was, the folder it went with): they come back
    /// with it.
    #[serde(default)]
    pub went_with: Vec<(String, String)>,
}

impl Done {
    /// How much room was freed.
    pub fn bytes(&self) -> u64 {
        total(self.removed.iter().map(|r| r.bytes))
    }
}

/// What [`Bin::send`] says when Windows wants administrator rights to remove something (files of a plugin's in a
/// folder it guards). The clean-ups then send all of those in one administrator step ([`Bin::admin`]).
pub const NEEDS_ADMIN: &str = "Windows needs administrator rights to remove this; remove it in Explorer instead.";

/// Where removed things go. The app uses [`SystemBin`]; tests use a folder of their own.
pub trait Bin {
    /// Puts a file or folder in the bin, or says what stopped it ([`NEEDS_ADMIN`] when Windows wants administrator
    /// rights for it).
    fn send(&self, path: &Path) -> Result<(), String>;
    /// Windows' administrator question, for what it guards: the app's bin asks it; by default nothing is asked.
    fn admin(&self) -> &dyn Admin {
        &NoAdmin
    }
    /// After a clean-up: finds each removed thing in the bin and notes what the bin calls it, so it can be put back.
    /// Says which aren't there (removed for good when Windows asked about something too big for the bin).
    fn find(&self, removed: &mut [Removed]) -> Vec<Problem>;
    /// Puts things back from the bin where they were; says what couldn't come back.
    fn put_back(&self, removed: &[Removed]) -> Vec<Problem>;
}

/// Sends each line of the plan to the bin, checking each again just before: one that changed, or that the guard now
/// refuses, stays. A copy only goes while the save it copies is still there and still the same. Once you press Stop,
/// what hasn't gone yet stays.
pub fn apply(plan: &Plan, guard: &Guard<'_>, bin: &dyn Bin, stop: &AtomicBool) -> Done {
    let mut done = Done::default();
    let mut sending = Sending::new(bin, stop);
    for line in &plan.lines {
        let item = &line.item;
        let check = || {
            if stop.load(Ordering::Relaxed) {
                return Err(Refused::Stopped);
            }
            match guard.check(item) {
                Err(why) => Err(why),
                Ok(now) if now.stamp != line.stamp => Err(Refused::Changed),
                Ok(now) => Ok(now.real),
            }
        };
        let real = match check() {
            Ok(real) => real,
            Err(why) => {
                done.problems.push(Problem { path: item.path.clone(), why: why.text().to_owned() });
                continue;
            }
        };
        let again = move || check().map(|_| ()).map_err(|why| why.text().to_owned());
        sending.send(&item.path, real, item.bytes, What::Song(item.kind), &mut done, again);
    }
    sending.finish(&mut done);
    done
}

/// What's said of something that's only online in OneDrive (all of it, or some of what's inside).
pub(crate) const ONLINE_ONLY: &str = "It's only online in OneDrive (all of it or some of it), so it can't go to the \
                                      Recycle Bin: removing it would delete it from OneDrive everywhere. It stays.";

/// Why the Recycle Bin couldn't keep something, so removing it would delete it for good: it's on a network drive, or a
/// USB stick or memory card (Windows), which have no Recycle Bin, or it's a OneDrive file that's only online, or a
/// folder holding one (only Windows has them, so only there is a folder walked). Each clean-up checks this itself first, where it can say more; [`Sending::send`]
/// checks it once more for all of them, whichever clean-up they come from.
pub(crate) fn cant_keep(path: &str, real: &str, stop: &AtomicBool) -> Option<&'static str> {
    if on_network(path) || on_network(real) {
        return Some(Refused::NetworkDrive.text());
    }
    if on_removable(real) {
        return Some(Refused::RemovableDrive.text());
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Refused::Gone.text()),
        Err(_) => return Some(CANT_LOOK),
    };
    if is_online_only(&meta) {
        return Some(ONLINE_ONLY);
    }
    if !meta.is_dir() {
        return None;
    }
    match online_only_in(Path::new(path), stop) {
        Ok(OnlineOnly::No) => None,
        Ok(OnlineOnly::Yes) => Some(ONLINE_ONLY),
        Ok(OnlineOnly::CantTell) => Some(CANT_LOOK),
        Err(Stopped) => Some(STOPPED),
    }
}

/// What's said of something that couldn't all be looked at to check the Recycle Bin can keep it.
const CANT_LOOK: &str = "Some of it couldn't be read to check it's all on this computer (not only online in OneDrive), \
                         so it stays.";

/// What's said of something that stays because you pressed Stop.
pub(crate) const STOPPED: &str = "Stopped before it went.";

/// What [`Bin::find`] says of something that went but isn't in the bin.
pub(crate) const NOT_IN_BIN: &str = "It isn't in the Recycle Bin, so it may have been removed for good.";

/// Sends checked things to the bin one at a time. Anything Windows guards waits, and all of it goes in one
/// administrator step ([`Sending::flush`]), so Windows asks once. [`Sending::finish`] sends what waits and then finds
/// everything removed in the bin: a clean-up ends with it, so nothing waiting can be left out of what it says.
pub(crate) struct Sending<'a> {
    bin: &'a dyn Bin,
    /// Stop: what isn't sent yet stays.
    stop: &'a AtomicBool,
    waiting: Vec<(Removed, Again<'a>)>,
    /// Where things the administrator step sent were (keys), for what's said when the bin doesn't have them.
    by_admin: Vec<String>,
    /// Set once Windows has been asked (it asks at most once in a clean-up): why anything else Windows guards stays.
    not_asked: Option<String>,
}

/// Checks something waiting for the administrator step again just before Windows asks: why it stays, if it does.
type Again<'a> = Box<dyn Fn() -> Result<(), String> + 'a>;

/// How sending one thing went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sent {
    /// It's in the bin.
    Went,
    /// Windows guards it: it waits for the administrator step.
    Waits,
    /// It stays, and `done` says why.
    Stays,
}

impl<'a> Sending<'a> {
    pub(crate) fn new(bin: &'a dyn Bin, stop: &'a AtomicBool) -> Self {
        Sending { bin, stop, waiting: Vec::new(), by_admin: Vec::new(), not_asked: None }
    }

    /// Sends one checked thing to the bin and notes how it went in `done`. One Windows guards waits for the
    /// administrator step, and `again` checks it again just before Windows asks (the other lines, and you, take time).
    pub(crate) fn send(
        &mut self,
        path: &str,
        real: String,
        bytes: u64,
        what: What,
        done: &mut Done,
        again: impl Fn() -> Result<(), String> + 'a,
    ) -> Sent {
        let stays = |done: &mut Done, why: String| {
            done.problems.push(Problem { path: path.to_owned(), why });
            Sent::Stays
        };
        // Whichever clean-up it's from, these hold. A link never goes: the Recycle Bin would take the link, but the
        // administrator step the place it leads to, which you didn't pick.
        if is_link(Path::new(path)) {
            return stays(done, Refused::Link.text().to_owned());
        }
        // The same thing reached another way goes once (it may be waiting for the administrator step).
        let same = |r: &Removed| key(&r.real) == key(&real);
        if let Some(first) = done.removed.iter().chain(self.waiting.iter().map(|(r, _)| r)).find(|r| same(r)) {
            let why = format!("It's the same as {}, reached another way, so it's only sent once.", first.path);
            return stays(done, why);
        }
        // What the Recycle Bin can't keep stays, as removing it would delete it for good.
        if let Some(why) = cant_keep(path, &real, self.stop) {
            return stays(done, why.to_owned());
        }
        if self.stop.load(Ordering::Relaxed) {
            return stays(done, STOPPED.to_owned());
        }
        let removed = Removed { path: path.to_owned(), real, bytes, what, when: now_seconds(), bin_id: None };
        let why = match self.bin.send(Path::new(path)) {
            Ok(()) => {
                done.removed.push(removed);
                return Sent::Went;
            }
            Err(why) if why == NEEDS_ADMIN && self.bin.admin().can_ask() => match &self.not_asked {
                Some(no) => no.clone(),
                None => {
                    self.waiting.push((removed, Box::new(again)));
                    return Sent::Waits;
                }
            },
            Err(why) => why,
        };
        stays(done, why)
    }

    /// Sends what waits to the bin in one administrator step (Windows asks you once). Each is checked again first, and
    /// its [`stamp`] taken, which the step checks once more before binning it. What's gone afterwards counts as removed;
    /// the rest stays, with why.
    pub(crate) fn flush(&mut self, done: &mut Done) {
        let mut lines = Vec::new();
        let mut sent = Vec::new();
        for (r, again) in std::mem::take(&mut self.waiting) {
            // The step is given where it really is: that must still be the very thing picked (in the same real
            // folder: one on the way may have been swapped for a link meanwhile), and not a link.
            let still_there = || {
                if self.stop.load(Ordering::Relaxed) {
                    return Err(STOPPED.to_owned());
                }
                if not_there(Path::new(&r.path)) {
                    return Err(Refused::Gone.text().to_owned());
                }
                if is_link(Path::new(&r.path)) {
                    return Err(Refused::Link.text().to_owned());
                }
                let folder = |p: &str| Path::new(p).parent().map(|f| key(&text(f)));
                if folder(&real_spot(&r.path)) != folder(&r.real) {
                    return Err("It's reached through a link that leads somewhere else now, so it stays.".to_owned());
                }
                Ok(())
            };
            let stamped = still_there().and_then(|()| again()).and_then(|()| {
                stamp(Path::new(&r.real))
                    .ok_or_else(|| "It couldn't all be read to check it again, so it stays.".to_owned())
            });
            match stamped {
                Ok(stamp) => {
                    lines.push(vec![r.real.clone(), stamp]);
                    sent.push(r);
                }
                Err(why) => done.problems.push(Problem { path: r.path, why }),
            }
        }
        if sent.is_empty() {
            return;
        }
        let when = now_seconds();
        let said = ask_for(self.bin.admin(), Step::Bin, &lines);
        self.not_asked = Some(match &said {
            Err(why) => why.clone(),
            Ok(_) => "Windows guards this too, and was asked once already in this clean-up: clean up again to be \
                      asked about it."
                .to_owned(),
        });
        for (n, r) in sent.into_iter().enumerate() {
            // What's there now decides, whatever the step said.
            if not_there(Path::new(&r.real)) {
                self.by_admin.push(key(&r.real));
                done.removed.push(Removed { when, ..r });
            } else {
                let why = reason(&said, n, "It's still there: a program may be using some of it.");
                done.problems.push(Problem { path: r.path, why });
            }
        }
    }

    /// Sends what waits ([`Sending::flush`]), then finds everything removed in the bin, so it can be put back.
    pub(crate) fn finish(mut self, done: &mut Done) {
        self.flush(done);
        for mut problem in self.bin.find(&mut done.removed) {
            let as_admin = |r: &Removed| key(&r.path) == key(&problem.path) && self.by_admin.contains(&key(&r.real));
            if problem.why == NOT_IN_BIN && done.removed.iter().any(as_admin) {
                problem.why = "It went to the Recycle Bin as administrator but isn't in yours: if Windows asked for \
                               another account's password, it's in that account's Recycle Bin."
                    .to_owned();
            }
            done.problems.push(problem);
        }
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
    fn admin(&self) -> &dyn Admin {
        &AsAdmin
    }

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
        trash::Error::Os { code, .. } if code & 0xFFFF == 5 || code & 0xFFFF == 0x78 => NEEDS_ADMIN.to_owned(),
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
    let real = Path::new(&r.real);
    let same_folder = real.parent().is_some_and(|f| key(&text(f)) == key(&text(&item.original_parent)));
    same_folder && same_name(item, real) && item.time_deleted >= r.when.saturating_sub(2)
}

/// Whether a bin item has the name of the file removed. Windows gives the name as Explorer shows it, so with "Hide
/// extensions for known file types" on (as it is at first) `Kick.wav` is listed as `Kick`: then the file in the bin
/// (`$R….wav`) has to have the same ending.
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn same_name(item: &trash::TrashItem, real: &Path) -> bool {
    let shown = key(&item.name.to_string_lossy());
    let ending = |p: &Path| p.extension().map(|e| key(&e.to_string_lossy()));
    real.file_name().is_some_and(|n| key(&n.to_string_lossy()) == shown)
        || (real.file_stem().is_some_and(|s| key(&s.to_string_lossy()) == shown)
            && ending(real).is_some()
            && ending(Path::new(&item.id)) == ending(real))
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
            None => problems.push(Problem { path: r.path.clone(), why: NOT_IN_BIN.into() }),
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
        // The one found when it went; only one never found then is looked for by place and time (an id that's gone
        // means it was put back already, and anything there now from that place went later).
        let item = match r.bin_id.as_deref() {
            Some(id) => items.iter().find(|i| i.id.to_str() == Some(id)),
            None => items.iter().filter(|i| is_the_one(i, r)).min_by_key(|i| i.time_deleted),
        };
        let Some(item) = item else {
            problems.push(problem(r, "It isn't in the Recycle Bin any more."));
            continue;
        };
        // It comes back under its own full name, even when Windows listed it without its ending.
        let mut item = item.clone();
        if let Some(name) = Path::new(&r.real).file_name().filter(|_| same_name(&item, Path::new(&r.real))) {
            item.name = name.to_owned();
        }
        match catch_unwind(AssertUnwindSafe(|| trash::os_limited::restore_all([item]))) {
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

#[cfg(test)]
mod tests {
    use super::on_network;

    #[test]
    fn a_server_path_is_on_the_network() {
        assert!(on_network(r"\\nas\music\Kontakt"));
        assert!(on_network("//nas/music/Kontakt"));
        // A folder that isn't there can't be asked about: only its path tells.
        assert!(!on_network("no such folder/Kontakt"));
    }
}
