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

use std::collections::{HashMap, HashSet};
use std::fs::{self, Metadata};
use std::io::Read;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::NaiveDateTime;
use mo_formats::case;
use mo_formats::paths::file_name;
use mo_formats::textscan::is_audio_path;
use mo_library::scan::{daw_of, is_inside, is_project_file};
use serde::{Deserialize, Serialize};

use crate::admin::{Admin, AsAdmin, NoAdmin, Outcome, Step, ask_for, real_spot, reason, stamp};
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
/// one, so removing from it deletes for good. (A Mac and Linux keep a Trash on each drive.)
#[cfg(windows)]
pub(crate) fn on_removable(real: &str) -> bool {
    // A list that can't be read says nothing is removable: as it was before this check.
    volume_of(real).is_some_and(|v| v.removable)
}

/// A drive in Windows' list of drives: where it is, whether it's a USB stick or memory card, and its kind ("NTFS",
/// "exFAT").
#[cfg(windows)]
#[derive(Clone)]
pub(crate) struct Volume {
    at: String,
    pub(crate) removable: bool,
    pub(crate) kind: String,
}

/// The drive a file or folder (where it really is) is on, from Windows' list of drives (the drives in the computer, and
/// USB sticks or memory cards: not network drives, nor CDs or DVDs): the innermost one holding it (a drive can sit in
/// a folder of another), or none when it isn't in the list (or the list can't be read). The list is read at most once a
/// second, as one look at hundreds of plugins asks about each, and so it's never more than a second old: a USB stick
/// plugged in where another drive was is known for what it is by the time a clean-up or a move gets to it.
#[cfg(windows)]
pub(crate) fn volume_of(real: &str) -> Option<Volume> {
    use std::sync::{Mutex, PoisonError};
    use std::time::{Duration, Instant};
    static READ: Mutex<Option<(Instant, Vec<Volume>)>> = Mutex::new(None);
    let mut read = READ.lock().unwrap_or_else(PoisonError::into_inner);
    if !read.as_ref().is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(1)) {
        let disks = sysinfo::Disks::new_with_refreshed_list_specifics(sysinfo::DiskRefreshKind::nothing());
        let volume = |d: &sysinfo::Disk| Volume {
            at: text(d.mount_point()),
            removable: d.is_removable(),
            kind: d.file_system().to_string_lossy().into_owned(),
        };
        let volumes = disks.map_or_else(|_| Vec::new(), |d| d.iter().map(volume).collect());
        *read = Some((Instant::now(), volumes));
    }
    let volumes = read.as_ref().map_or(&[][..], |(_, volumes)| volumes.as_slice());
    volumes.iter().filter(|v| is_inside(real, &v.at)).max_by_key(|v| v.at.len()).cloned()
}

/// A Mac and Linux keep a Trash on each drive, a USB stick too.
#[cfg(not(windows))]
pub(crate) fn on_removable(_real: &str) -> bool {
    false
}

/// A guard with where its folders really are worked out once, for one preview or one clean-up: each line would
/// otherwise look up every project and sample folder again (slow with thousands of lines and a folder on the network).
/// A project or sample folder swapped for a link part way through isn't seen; each line's own place still is, and a
/// line reached through a link that leads out of its project folder still stays.
struct Prepared<'g> {
    guard: &'g Guard<'g>,
    roots: Vec<(&'g str, String)>,
    samples: Vec<(&'g str, String)>,
}

impl<'g> Prepared<'g> {
    fn new(guard: &'g Guard<'g>) -> Self {
        Prepared {
            guard,
            roots: guard.project_roots.iter().map(|r| both(r)).collect(),
            samples: guard.sample_folders.iter().map(|f| both(f)).collect(),
        }
    }

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
        let roots = &self.roots;
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
        let in_samples = self.samples.iter().any(|(f, real_f)| {
            is_inside(path, f) || is_inside(f, path) || is_inside(&real, real_f) || is_inside(real_f, &real)
        });
        if in_samples {
            return Err(Refused::InSampleFolder);
        }
        if self.guard.keep.contains(&key(path)) || self.guard.keep.contains(&key(&real)) {
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
                // Only the folders below the project folder count: a project folder called "auto-backups" doesn't make
                // every song in it an autosave. And only what the DAW made by itself, as the measure lists them: a save
                // of yours in a folder called "Backup" is yours.
                let all: Vec<&str> = parts(path).collect();
                let below_root = all.get(parts(root).count()..all.len().saturating_sub(1)).unwrap_or_default();
                let below_root: Vec<String> = below_root.iter().map(|d| (*d).to_owned()).collect();
                if !meta.is_file() || daw_of(path).is_none() || !crate::song::is_daws_autosave(&below_root, name) {
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
                if (self.guard.sample_in_use)(name) {
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
    let (mut fa, mut fb) = (crate::size::open_to_read_once(a)?, crate::size::open_to_read_once(b)?);
    if fa.metadata()?.len() != fb.metadata()?.len() {
        return Ok(false);
    }
    let same = same_reads(&mut fa, &mut fb);
    crate::size::read_once_done(&fa);
    crate::size::read_once_done(&fb);
    same
}

/// Whether two open files of the same size read the same, in pieces.
fn same_reads(fa: &mut fs::File, fb: &mut fs::File) -> std::io::Result<bool> {
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
    let guard = Prepared::new(guard);
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
    /// When the bin says it went in (Unix seconds), once found there. Linux names a thing in the Trash after the one
    /// before it once that's gone, so a later thing of the same name from the same place can be called the same: this
    /// tells them apart.
    #[serde(default)]
    pub binned: Option<i64>,
    /// On Linux, the computer's time zone when it was found there (seconds ahead of UTC): the Trash keeps the time as
    /// the clock showed it, without the zone, so once the computer is in another zone it reads that much off.
    #[serde(default)]
    pub binned_zone: Option<i32>,
    /// What it held when it went as administrator (its [`stamp`]): it comes back the same way, Windows asking once for
    /// all of a clean-up's, and only while what's in the bin still holds this. `None` for what went without asking.
    #[serde(default)]
    pub stamp: Option<String>,
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
    /// An F12 note's words or picture, about 3 months old, put there by the app's start-up clean-up (rule 28).
    Note,
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

/// What Put back says when Windows wants administrator rights to put something back and the app can't ask for them:
/// it went without asking (so the app didn't note what it held), or nothing here can ask.
pub const BACK_NEEDS_ADMIN: &str =
    "Windows needs administrator rights to put this back; restore it from the Recycle Bin instead.";

/// What the administrator step's Put back looks at just before something comes back ([`Bin::put_back_checked`]): what
/// went, and the file or folder the bin keeps for it now. `Err` says why it stays in the bin.
pub type Check<'a> = dyn Fn(&Removed, &Path) -> Result<(), String> + 'a;

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
    /// Which of these are in the bin now, in the same order (not once you've emptied the bin or put them back), or
    /// `None` when the bin can't be looked in. By default it can't.
    fn holds(&self, _removed: &[Removed]) -> Option<Vec<bool>> {
        None
    }
    /// The administrator step's Put back: puts back from the bin, one by one, those of `removed` that `check` lets
    /// through, each looked at where the bin keeps it (the file or folder itself) just before it comes back, and tells
    /// `each` how each went as soon as it's done (by where it is in `removed`): `None` when it came back, or why not.
    /// The bin is read once for all of it. By default it can't be read, so nothing comes back.
    fn put_back_checked(&self, removed: &[Removed], _check: &Check<'_>, each: &mut dyn FnMut(usize, Option<String>)) {
        for n in 0..removed.len() {
            each(n, Some(CANT_READ_BIN.to_owned()));
        }
    }
    /// Whether the bin `removed` went to can be looked in now: not while the drive it was on isn't plugged in (a USB
    /// disk keeps its own Recycle Bin), when [`holds`](Self::holds) doesn't see it though it may still be there for when
    /// the drive is back. By default, always.
    fn can_look_for(&self, _removed: &Removed) -> bool {
        true
    }
    /// Why this bin can't take `path` (`bytes` big) quietly: sending it would ask you something first, or remove it for
    /// good. `None` when it can. For what the app clears by itself, with nobody there to answer (the start-up sweep's
    /// old F12 notes); the Space page's clean-ups, which you start and watch, check what they need as they go. A bin of
    /// the tests' own (a folder) takes everything.
    fn refuses_quietly(&self, _path: &Path, _bytes: u64) -> Option<String> {
        None
    }
}

/// What a clean-up hears of each send ([`Heard`]): where, and how it went.
pub type Hear<'a> = dyn Fn(&Path, &Result<(), String>) + Sync + 'a;

/// A bin that tells `hear` how each send went, so a clean-up's card can count what has gone so far. Everything else is
/// `bin`'s own: each of [`Bin`]'s ways is passed on to it (a test checks each one), so a way added to [`Bin`] later must
/// be passed on here too, or its default would stand in for `bin`'s own.
pub struct Heard<'a> {
    bin: &'a (dyn Bin + Sync),
    hear: &'a Hear<'a>,
}

impl<'a> Heard<'a> {
    pub fn new(bin: &'a (dyn Bin + Sync), hear: &'a Hear<'a>) -> Self {
        Heard { bin, hear }
    }
}

impl Bin for Heard<'_> {
    fn send(&self, path: &Path) -> Result<(), String> {
        let sent = self.bin.send(path);
        (self.hear)(path, &sent);
        sent
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
    fn put_back_checked(&self, removed: &[Removed], check: &Check<'_>, each: &mut dyn FnMut(usize, Option<String>)) {
        self.bin.put_back_checked(removed, check, each);
    }
    fn can_look_for(&self, removed: &Removed) -> bool {
        self.bin.can_look_for(removed)
    }
    fn refuses_quietly(&self, path: &Path, bytes: u64) -> Option<String> {
        self.bin.refuses_quietly(path, bytes)
    }
}

/// Sends each line of the plan to the bin, checking each again just before: one that changed, or that the guard now
/// refuses, stays. A copy only goes while the save it copies is still there and still the same. Once you press Stop,
/// what hasn't gone yet stays.
pub fn apply(plan: &Plan, guard: &Guard<'_>, bin: &dyn Bin, stop: &AtomicBool) -> Done {
    let guard = Prepared::new(guard);
    let guard = &guard;
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
pub const ONLINE_ONLY: &str = "It's only online in OneDrive (all of it or some of it), so it can't go to the \
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
pub const CANT_LOOK: &str = "Some of it couldn't be read to check it's all on this computer (not only online in OneDrive), \
                         so it stays.";

/// What's said of something that stays because you pressed Stop.
pub(crate) const STOPPED: &str = "Stopped before it went.";

/// What [`Bin::find`] says of something that went but isn't in the bin.
pub const NOT_IN_BIN: &str = "It isn't in the Recycle Bin, so it may have been removed for good.";

/// What [`Bin::send`] says when Windows didn't put it in the Recycle Bin and didn't say why: it asked first and was
/// told no, or something stopped it that the app can't see (with nobody to answer, Windows lets an error pass in
/// silence, and says the same as for a no). A file or folder a program has open, or one that needs administrator rights,
/// is told apart from this (see [`why_kept`]). Also what's said when Windows did say why, and it was that: the question
/// was cancelled, or its Recycle Bin couldn't take it (see [`bin_problem`]); that isn't looked into further.
pub const KEPT_BY_YOU: &str = "It stays: Windows asked first and was told no, or couldn't move it.";

/// What [`Bin::send`] says of a file or folder a program is using.
pub const IN_USE: &str = "A program is using it. Close your DAW and try again.";

/// What [`Bin::send`] says when Windows said it moved something to the Recycle Bin but it's still there: a program is
/// using some of it (a folder), or Windows asked first and was told no, or, told yes, removed it for good while a
/// program still had it open, so that it lingers for a moment. The start-up sweep's notes stop going by themselves on
/// it, as on [`KEPT_BY_YOU`].
pub const STILL_THERE: &str = "It's still there: you chose to keep it, or a program is using some of it.";

/// Sends checked things to the bin one at a time. Anything Windows guards waits, and all of it goes in one
/// administrator step ([`Sending::flush`]), so Windows asks once. [`Sending::finish`] sends what waits and then finds
/// everything removed in the bin: a clean-up ends with it, so nothing waiting can be left out of what it says.
pub(crate) struct Sending<'a> {
    bin: &'a dyn Bin,
    /// Stop: what isn't sent yet stays.
    stop: &'a AtomicBool,
    waiting: Vec<(Removed, Again<'a>)>,
    /// What was sent or is waiting for the administrator step, where it really is (as [`key`]s), and as it was shown:
    /// to send the same thing reached another way only once, without comparing it with every line before it.
    sent: HashMap<String, String>,
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
        Sending { bin, stop, waiting: Vec::new(), sent: HashMap::new(), by_admin: Vec::new(), not_asked: None }
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
        let real_key = key(&real);
        if let Some(first) = self.sent.get(&real_key) {
            let why = format!("It's the same as {first}, reached another way, so it's only sent once.");
            return stays(done, why);
        }
        // What the Recycle Bin can't keep stays, as removing it would delete it for good.
        if let Some(why) = cant_keep(path, &real, self.stop) {
            return stays(done, why.to_owned());
        }
        if self.stop.load(Ordering::Relaxed) {
            return stays(done, STOPPED.to_owned());
        }
        let removed = Removed {
            path: path.to_owned(),
            real,
            bytes,
            what,
            when: now_seconds(),
            bin_id: None,
            binned: None,
            binned_zone: None,
            stamp: None,
        };
        let why = match self.bin.send(Path::new(path)) {
            Ok(()) => {
                self.sent.insert(real_key, removed.path.clone());
                done.removed.push(removed);
                return Sent::Went;
            }
            Err(why) if why == NEEDS_ADMIN && self.bin.admin().can_ask() => match &self.not_asked {
                Some(no) => no.clone(),
                None => {
                    self.sent.insert(real_key, removed.path.clone());
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
                Err(why) => {
                    // It didn't go: the same thing reached another way may still try.
                    self.sent.remove(&key(&r.real));
                    done.problems.push(Problem { path: r.path, why });
                }
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
        for (n, (r, line)) in sent.into_iter().zip(lines).enumerate() {
            // What's there now decides, whatever the step said.
            if not_there(Path::new(&r.real)) {
                self.by_admin.push(key(&r.real));
                // The stamp the step checked before binning it: what's in the bin, for Put back to check.
                let stamp = line.into_iter().nth(1);
                done.removed.push(Removed { when, stamp, ..r });
            } else {
                let why = reason(&said, n, "It's still there: a program may be using some of it.");
                self.sent.remove(&key(&r.real));
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

/// Puts back what a clean-up removed. What went as administrator ([`Removed::stamp`]: Windows guards its place) comes
/// back the same way, all of it in one administrator step, so Windows asks once; the rest comes back as you.
pub fn undo(done: &Done, bin: &dyn Bin) -> Vec<Problem> {
    let asks = bin.admin().can_ask();
    let (guarded, plain): (Vec<Removed>, Vec<Removed>) =
        done.removed.iter().cloned().partition(|r| asks && r.stamp.is_some());
    let mut problems = if plain.is_empty() { Vec::new() } else { bin.put_back(&plain) };
    if !guarded.is_empty() {
        problems.extend(put_back_guarded(&guarded, bin));
    }
    problems
}

/// Puts back in one administrator step what went as administrator. Windows is asked only about what can come back:
/// what the bin still holds (when it can be looked in), with nothing at its place now. Each line is where it was and
/// what the app keeps of it (as JSON, with its stamp), which the step checks against the bin before putting it back.
/// Afterwards, what's there decides what came back, as long as the step didn't say it left it in the bin.
fn put_back_guarded(guarded: &[Removed], bin: &dyn Bin) -> Vec<Problem> {
    let problem = |r: &Removed, why: &str| Problem { path: r.path.clone(), why: why.to_owned() };
    let mut problems = Vec::new();
    let held = bin.holds(guarded);
    let mut asking = Vec::new();
    for (n, r) in guarded.iter().enumerate() {
        if held.as_ref().is_some_and(|held| held.get(n) == Some(&false)) {
            problems.push(problem(r, GONE_FROM_BIN));
        } else if !not_there(Path::new(&r.real)) {
            problems.push(problem(r, NAME_TAKEN));
        } else {
            asking.push(r.clone());
        }
    }
    if asking.is_empty() {
        return problems;
    }
    // A line the step can't read (none the app writes) is one it leaves, saying so.
    let lines: Vec<Vec<String>> =
        asking.iter().map(|r| vec![r.real.clone(), serde_json::to_string(r).unwrap_or_default()]).collect();
    let said = ask_for(bin.admin(), Step::PutBack, &lines);
    let held = bin.holds(&asking);
    for (n, r) in asking.iter().enumerate() {
        let left = matches!(&said, Ok(outcomes) if outcomes.get(n) != Some(&Outcome::Done));
        let here = !not_there(Path::new(&r.real));
        let out_of_bin = held.as_ref().is_none_or(|held| held.get(n) == Some(&false));
        if left || !here || !out_of_bin {
            problems.push(problem(r, &reason(&said, n, COULDNT_PUT_BACK)));
        }
    }
    problems
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
        let sent = mo_catch::catch(AssertUnwindSafe(|| trash::delete(path)))
            .map_err(|_| "Windows wouldn't start moving it to the Recycle Bin.".to_owned())?;
        match sent {
            Ok(()) if fs::symlink_metadata(path).is_ok() => Err(STILL_THERE.to_owned()),
            Ok(()) => Ok(()),
            Err(e) => Err(problem_with(&e, path)),
        }
    }

    fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
        find_in_system_bin(removed)
    }

    fn put_back(&self, removed: &[Removed]) -> Vec<Problem> {
        let mut problems = Vec::new();
        put_back_from_system_bin(removed, None, &mut |n, why| {
            if let Some(why) = why {
                problems.push(Problem { path: removed[n].path.clone(), why });
            }
        });
        problems
    }

    fn put_back_checked(&self, removed: &[Removed], check: &Check<'_>, each: &mut dyn FnMut(usize, Option<String>)) {
        put_back_from_system_bin(removed, Some(check), each);
    }

    fn holds(&self, removed: &[Removed]) -> Option<Vec<bool>> {
        held_in_system_bin(removed)
    }

    fn can_look_for(&self, removed: &Removed) -> bool {
        drive_is_here(&removed.real)
    }

    fn refuses_quietly(&self, path: &Path, bytes: u64) -> Option<String> {
        refuses_quietly(path, bytes).map(str::to_owned)
    }
}

/// Whether the drive `real` is on is plugged in, so its Recycle Bin can be looked in: on Windows, the drive's own top
/// folder (I: itself); elsewhere, where drives have no letters, the folder it was in (a USB disk's folders are only
/// there while it is).
fn drive_is_here(real: &str) -> bool {
    let real = Path::new(real);
    let at = if cfg!(windows) { real.ancestors().last() } else { real.parent() };
    at.is_some_and(|at| !at.as_os_str().is_empty() && at.exists())
}

/// Why the computer's own Recycle Bin can't take `path` (`bytes` big) without a word: what [`cant_keep`] looks for (a
/// network drive, a USB stick or memory card, a file only online in OneDrive), without walking a folder; or, on
/// Windows, what [`bin_refuses`] looks for. Windows asks "permanently delete?" about a file it can't keep, and removes
/// it for good on a yes. On a Mac the Trash is reached through Finder, which can ask first too.
fn refuses_quietly(path: &Path, bytes: u64) -> Option<&'static str> {
    let shown = text(path);
    let (shown, real) = both(&shown);
    if on_network(shown) || on_network(&real) {
        return Some(Refused::NetworkDrive.text());
    }
    if on_removable(&real) {
        return Some(Refused::RemovableDrive.text());
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if is_online_only(&meta) => return Some(ONLINE_ONLY),
        Ok(_) => {}
        Err(_) => return Some(CANT_LOOK),
    }
    bin_refuses(Path::new(&real), bytes)
}

/// What's said of something that stays because the Recycle Bin is off where it is.
#[cfg_attr(not(windows), allow(dead_code))]
const BIN_OFF: &str = "The Recycle Bin is off for that drive, so it would be removed for good. It stays.";
/// What's said of something the Recycle Bin hasn't room for.
#[cfg_attr(not(windows), allow(dead_code))]
const NO_ROOM: &str = "The Recycle Bin on that drive is nearly full (or smaller than it), so putting it there would \
                       remove it, or the oldest things in the Recycle Bin, for good. It stays.";
/// What's said of something on a drive Windows doesn't name.
#[cfg(windows)]
const UNKNOWN_DRIVE: &str =
    "Windows didn't say which drive it's on, so whether the Recycle Bin can take it isn't known. It stays.";
/// What's said of something that isn't on one of the computer's own drives.
#[cfg_attr(not(windows), allow(dead_code))]
const NOT_OWN_DRIVE: &str =
    "It isn't on one of this computer's own drives, so the Recycle Bin may not keep it. It stays.";
/// What's said when how full the Recycle Bin is can't be read.
#[cfg_attr(not(windows), allow(dead_code))]
const CANT_SEE_ROOM: &str = "How full the Recycle Bin on that drive is couldn't be read, so it stays.";
/// What Windows calls one of the computer's own drives (a hard drive or SSD inside it, or a USB one plugged in),
/// rather than a USB stick, memory card, CD, network drive or RAM drive.
#[cfg_attr(not(windows), allow(dead_code))]
const DRIVE_FIXED: u32 = 3;

/// Windows: whether the Recycle Bin can't take `real` (`bytes` big) without asking. It can't when the Recycle Bin is
/// off (a policy for this account or the whole computer, or that drive's own setting), when it isn't one of the
/// computer's own drives, or when the Recycle Bin there hasn't room: past its size Windows asks about something too
/// big, and makes room for the rest by removing the oldest things in it for good. One that can't be told about stays.
/// What Windows says is read here; [`bin_decides`] works it out.
#[cfg(windows)]
fn bin_refuses(real: &Path, bytes: u64) -> Option<&'static str> {
    use windows_registry::{CURRENT_USER, LOCAL_MACHINE};
    // Where policies can say "Do not move deleted files to the Recycle Bin" and "Maximum allowed Recycle Bin size".
    const POLICY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
    // Each drive's Recycle Bin settings (the Recycle Bin's Properties), under the drive's id.
    const DRIVES: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume";
    // A path Windows can't put on a drive (a `\\?\Volume{…}\` one, once its `\\?\` is taken off) stays.
    if !real.is_absolute() {
        return Some(UNKNOWN_DRIVE);
    }
    let policies = |name: &str| -> Vec<u32> {
        [LOCAL_MACHINE, CURRENT_USER]
            .into_iter()
            .filter_map(|hive| hive.open(POLICY).ok()?.get_u32(name).ok())
            .collect()
    };
    let Some(drive) = Drive::of(real) else { return Some(UNKNOWN_DRIVE) };
    // A drive whose Recycle Bin settings were never changed may have none of its own: then Windows' own hold.
    let settings = CURRENT_USER.open(format!(r"{DRIVES}\{}", drive.id)).ok();
    let setting = |name: &str| settings.as_ref().and_then(|s| s.get_u32(name).ok());
    let facts = BinFacts {
        no_recycle_policy: policies("NoRecycleFiles").into_iter().any(|on| on != 0),
        kind: drive.kind,
        nuke_on_delete: setting("NukeOnDelete").is_some_and(|on| on != 0),
        size: drive.size,
        policy_percent: policies("RecycleBinSize").into_iter().min(),
        max_mb: setting("MaxCapacity"),
    };
    bin_decides(&facts, bytes, || drive.bin_holds())
}

/// What Windows says of a drive and its Recycle Bin, as [`bin_refuses`] reads it.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy)]
struct BinFacts {
    /// A policy says "Do not move deleted files to the Recycle Bin", for this account or the whole computer.
    no_recycle_policy: bool,
    /// What kind of drive it is ([`DRIVE_FIXED`] for one of the computer's own).
    kind: u32,
    /// The drive's own "Don't move files to the Recycle Bin. Remove files immediately when deleted."
    nuke_on_delete: bool,
    /// How big the drive is, in bytes.
    size: u64,
    /// A policy's "Maximum allowed Recycle Bin size", as a share of the drive in percent (the smaller of two).
    policy_percent: Option<u32>,
    /// The drive's own "Maximum size", in MB.
    max_mb: Option<u32>,
}

/// Whether the Recycle Bin can't take `bytes` more without asking, by what Windows says of it (`facts`); `held` reads
/// what's in it now, only when that's needed. The bin may hold a policy's share of the drive, else that drive's own
/// maximum size, else a twentieth of the drive (Windows' own first choice is more); past nine tenths of that, it stays.
#[cfg_attr(not(windows), allow(dead_code))]
fn bin_decides(facts: &BinFacts, bytes: u64, held: impl FnOnce() -> Option<u64>) -> Option<&'static str> {
    if facts.no_recycle_policy || facts.nuke_on_delete {
        return Some(BIN_OFF);
    }
    if facts.kind != DRIVE_FIXED {
        return Some(NOT_OWN_DRIVE);
    }
    let most = match (facts.policy_percent, facts.max_mb) {
        (Some(percent), _) => facts.size / 100 * u64::from(percent.min(100)),
        (None, Some(mb)) => u64::from(mb).saturating_mul(1024 * 1024),
        (None, None) => facts.size / 20,
    };
    // What's in it now, with a tenth to spare.
    let Some(held) = held() else { return Some(CANT_SEE_ROOM) };
    (held.saturating_add(bytes) > most / 10 * 9).then_some(NO_ROOM)
}

/// The drive holding a file, as Windows knows it.
#[cfg(windows)]
struct Drive {
    /// Where it starts ("C:\", or the folder a drive is mounted in), ending in a 0 as Windows reads it.
    root: Vec<u16>,
    /// The id its Recycle Bin settings are kept under (the `{…}` of `\\?\Volume{…}\`).
    id: String,
    /// What kind of drive it is ([`DRIVE_FIXED`] for one of the computer's own).
    kind: u32,
    /// How big it is, in bytes.
    size: u64,
}

/// Windows: where the drive holding `real` starts ("C:\", or the folder a drive is mounted in, ending in a 0 as Windows
/// reads it), and its id (the `{…}` of `\\?\Volume{…}\`): the same id is the same drive, whatever letter or folder it's
/// reached by. `None` when Windows can't say.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) fn volume_of_path(real: &Path) -> Option<(Vec<u16>, String)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetVolumeNameForVolumeMountPointW, GetVolumePathNameW};
    let path: Vec<u16> = real.as_os_str().encode_wide().chain([0]).collect();
    // The drive's root is never longer than the path inside it.
    let mut root = vec![0u16; path.len().max(261)];
    let room = u32::try_from(root.len()).ok()?;
    // SAFETY: `path` ends in a 0, and `root` has room for `room` characters, as the call is told.
    if unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), room) } == 0 {
        return None;
    }
    let end = root.iter().position(|&c| c == 0)?;
    root.truncate(end + 1);
    // "\\?\Volume{…}\" is 49 characters and the 0 after them.
    let mut name = [0u16; 64];
    // SAFETY: `root` ends in a 0, and `name` has room for the 64 characters the call is told.
    if unsafe { GetVolumeNameForVolumeMountPointW(root.as_ptr(), name.as_mut_ptr(), 64) } == 0 {
        return None;
    }
    let end = name.iter().position(|&c| c == 0)?;
    let name = String::from_utf16(name.get(..end)?).ok()?;
    let id = name.strip_prefix(r"\\?\Volume")?.strip_suffix('\\')?;
    if !(id.starts_with('{') && id.ends_with('}')) {
        return None;
    }
    Some((root, id.to_owned()))
}

#[cfg(windows)]
impl Drive {
    /// The drive holding `real`; `None` when Windows can't say.
    #[allow(unsafe_code)]
    fn of(real: &Path) -> Option<Drive> {
        use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW};
        let (root, id) = volume_of_path(real)?;
        // SAFETY: `root` ends in a 0.
        let kind = unsafe { GetDriveTypeW(root.as_ptr()) };
        let mut size = 0u64;
        // SAFETY: `root` ends in a 0, `size` is a number to write to, and the two left out may be, as the call allows.
        if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), std::ptr::null_mut(), &mut size, std::ptr::null_mut()) } == 0 {
            return None;
        }
        Some(Drive { root, id, kind, size })
    }

    /// How much is in the Recycle Bin on this drive now (this account's), when Windows says. Read again at most every
    /// 5 seconds, as the drives are in [`on_removable`]: Windows counts it by going through the whole Recycle Bin, and
    /// the start-up sweep asks about each note it sends.
    #[allow(unsafe_code)]
    fn bin_holds(&self) -> Option<u64> {
        use std::sync::{Mutex, PoisonError};
        use std::time::{Duration, Instant};
        use windows_sys::Win32::UI::Shell::{SHQUERYRBINFO, SHQueryRecycleBinW};
        /// The drive last asked about, and what its Recycle Bin held then.
        static READ: Mutex<Option<(Instant, Vec<u16>, u64)>> = Mutex::new(None);
        let mut read = READ.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, root, held)) = read.as_ref()
            && at.elapsed() < Duration::from_secs(5)
            && *root == self.root
        {
            return Some(*held);
        }
        let size = u32::try_from(std::mem::size_of::<SHQUERYRBINFO>()).ok()?;
        let mut info = SHQUERYRBINFO { cbSize: size, ..Default::default() };
        // SAFETY: `root` ends in a 0, and `info` is as big as it says it is.
        if unsafe { SHQueryRecycleBinW(self.root.as_ptr(), &mut info) } != 0 {
            return None;
        }
        let held = u64::try_from(info.i64Size).ok()?;
        *read = Some((Instant::now(), self.root.clone(), held));
        Some(held)
    }
}

/// A Mac: the Trash is reached through Finder, which can first ask you to let the app use it.
#[cfg(target_os = "macos")]
fn bin_refuses(_real: &Path, _bytes: u64) -> Option<&'static str> {
    Some("On a Mac the Trash is reached through Finder, which can ask you first, so nothing goes there by itself.")
}

/// Linux keeps its Trash in folders of its own, and never asks.
#[cfg(not(any(windows, target_os = "macos")))]
fn bin_refuses(_real: &Path, _bytes: u64) -> Option<&'static str> {
    None
}

/// Windows: why a file or folder Windows didn't move without saying why ([`KEPT_BY_YOU`]) stayed, when it tells: a
/// program has it open and won't let it be removed ([`IN_USE`]), or it needs administrator rights ([`NEEDS_ADMIN`]:
/// a folder in Program Files, say, which then goes in the administrator step). It's only opened to see, asking for
/// nothing but the right to remove it, and closed again; nothing in it changes. A folder only tells of itself: a file
/// in use anywhere inside it doesn't show, so that one still counts as Windows having been told no.
#[cfg(windows)]
fn why_kept(path: &Path) -> Option<&'static str> {
    use std::os::windows::fs::OpenOptionsExt;
    // The right to remove it (DELETE), sharing everything with whoever else has it open: only a program that holds
    // it and won't let it be removed stops this.
    const DELETE: u32 = 0x0001_0000;
    const SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;
    // A folder can only be opened with this. It asks for no backup rights: the app is never given any.
    const BACKUP_SEMANTICS: u32 = 0x0200_0000;
    // A link is opened itself, never followed (none gets this far: links never go to the bin).
    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    // What Windows says of something another program won't let go of, and of something this account may not remove.
    const SHARING_VIOLATION: i32 = 32;
    const ACCESS_DENIED: i32 = 5;
    let kind = fs::symlink_metadata(path).ok()?;
    let flags = match (kind.is_dir(), kind.is_file()) {
        (true, _) => BACKUP_SEMANTICS,
        (_, true) => 0,
        _ => return None,
    };
    let opened = fs::OpenOptions::new()
        .access_mode(DELETE)
        .share_mode(SHARE_ALL)
        .custom_flags(flags | OPEN_REPARSE_POINT)
        .open(path);
    match opened {
        Err(e) if e.raw_os_error() == Some(SHARING_VIOLATION) => Some(IN_USE),
        Err(e) if e.raw_os_error() == Some(ACCESS_DENIED) => Some(NEEDS_ADMIN),
        _ => None,
    }
}

/// Elsewhere the bin never says it was told no.
#[cfg(not(windows))]
fn why_kept(_path: &Path) -> Option<&'static str> {
    None
}

/// What stopped `path` going to the Recycle Bin (`e`, from `trash`). When Windows didn't say why, the file or folder may
/// tell ([`why_kept`]); when it did (the question was cancelled, or its Recycle Bin couldn't take it), that stands, even
/// if a program has it open too.
fn problem_with(e: &trash::Error, path: &Path) -> String {
    let why = bin_problem(e);
    if why == KEPT_BY_YOU && matches!(e, trash::Error::Unknown { .. }) {
        return why_kept(path).map_or(why, str::to_owned);
    }
    why
}

/// What an error from `trash` means here. Windows' own errors (HRESULTs) are told apart by their whole number, as
/// which part of Windows a code comes from counts too.
fn bin_problem(e: &trash::Error) -> String {
    /// A program has it open and won't let it go (the sharing and lock violations, and the copy engine's own word).
    const SHARING_VIOLATION: [u32; 3] = [0x8007_0020, 0x8007_0021, 0x8027_0027];
    /// Windows didn't move it: it was told no or cancelled (ERROR_CANCELLED, E_ABORT, COPYENGINE_E_USER_CANCELLED,
    /// COPYENGINE_E_CANCELLED), or the Recycle Bin couldn't take it (COPYENGINE_E_RECYCLE_UNKNOWN_ERROR, _FORCE_NUKE,
    /// _SIZE_TOO_BIG, _PATH_TOO_LONG, _BIN_NOT_FOUND). The start-up sweep's notes stop going by themselves on these, the
    /// safe side when it may have asked.
    const NOT_MOVED: [u32; 4] = [0x8007_04C7, 0x8000_4004, 0x8027_0000, 0x8027_0001];
    const RECYCLE_BIN_COULDNT: std::ops::RangeInclusive<u32> = 0x8027_0035..=0x8027_003A;
    match e {
        trash::Error::Os { code, .. } => {
            let code = code.cast_unsigned();
            // Access denied, or it needs administrator rights: these go to the administrator step. (`trash` asks
            // Windows' newer file operations, whose errors are all HRESULTs: the bare 0x78 that its older one gave
            // for access denied never comes.)
            if NEEDS_RIGHTS.contains(&code) {
                NEEDS_ADMIN.to_owned()
            } else if SHARING_VIOLATION.contains(&code) {
                IN_USE.to_owned()
            } else if NOT_MOVED.contains(&code) || RECYCLE_BIN_COULDNT.contains(&code) {
                KEPT_BY_YOU.to_owned()
            } else {
                format!("It couldn't go to the Recycle Bin ({e:?}).")
            }
        }
        // What `trash` says when Windows didn't move something and didn't say why (5.2.9: "Some operations were
        // aborted"). On Windows it says `Unknown` for nothing else, so the kind of error decides, not its words. The
        // start-up sweep's notes stop going by themselves on it, so `trash` is held at that version (mo-space's
        // Cargo.toml): check this before updating it.
        trash::Error::Unknown { .. } if cfg!(windows) => KEPT_BY_YOU.to_owned(),
        trash::Error::CouldNotAccess { .. } => "It couldn't be reached.".to_owned(),
        _ => format!("It couldn't go to the Recycle Bin ({e:?})."),
    }
}

/// The bin's items by where each was before it went (its folder and its name, as [`key`]s) and by what the bin calls
/// it, read once: a clean-up can send thousands of things to a bin that holds thousands more, and looking through all
/// of the bin for each one took minutes. It only narrows down where to look: what's looked for there is as before.
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
struct BinIndex<'a> {
    items: &'a [trash::TrashItem],
    by_place: HashMap<(String, String), Vec<usize>>,
    by_id: HashMap<&'a std::ffi::OsStr, Vec<usize>>,
}

#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
impl<'a> BinIndex<'a> {
    fn new(items: &'a [trash::TrashItem]) -> Self {
        let mut by_place: HashMap<(String, String), Vec<usize>> = HashMap::new();
        let mut by_id: HashMap<&std::ffi::OsStr, Vec<usize>> = HashMap::new();
        for (n, item) in items.iter().enumerate() {
            let place = (key(&text(&item.original_parent)), key(&item.name.to_string_lossy()));
            by_place.entry(place).or_default().push(n);
            by_id.entry(item.id.as_os_str()).or_default().push(n);
        }
        BinIndex { items, by_place, by_id }
    }

    /// Every item that may be from where `r` was, in the bin's order: from its folder, called by its name or by its
    /// name without its ending, as [`from_there`] has it (which still says whether each is).
    fn near(&self, r: &Removed) -> impl Iterator<Item = &'a trash::TrashItem> + use<'a> {
        let real = Path::new(&r.real);
        let mut at: Vec<usize> = Vec::new();
        if let Some(folder) = real.parent().map(|f| key(&text(f))) {
            for name in [real.file_name(), real.file_stem()].into_iter().flatten() {
                at.extend(self.by_place.get(&(folder.clone(), key(&name.to_string_lossy()))).into_iter().flatten());
            }
        }
        at.sort_unstable();
        at.dedup();
        let items = self.items;
        at.into_iter().filter_map(move |n| items.get(n))
    }

    /// The items the bin calls `id`, in its order.
    fn called(&self, id: &std::ffi::OsStr) -> impl Iterator<Item = &'a trash::TrashItem> + use<'a> {
        let items = self.items;
        let at = self.by_id.get(id).cloned().unwrap_or_default();
        at.into_iter().filter_map(move |n| items.get(n))
    }
}

/// Whether a bin item is the one removed: from the same place, and gone to the bin at or after the moment it was sent
/// (a couple of seconds' leeway for the clock).
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn is_the_one(item: &trash::TrashItem, r: &Removed) -> bool {
    from_there(item, r) && item.time_deleted >= r.when.saturating_sub(2)
}

/// Whether a bin item is from where the thing removed was: the same folder, and the same name.
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn from_there(item: &trash::TrashItem, r: &Removed) -> bool {
    let real = Path::new(&r.real);
    real.parent().is_some_and(|f| key(&text(f)) == key(&text(&item.original_parent))) && same_name(item, real)
}

/// Whether a bin item went in when the bin said, when it was found there (`binned`): Linux names a thing in the Trash
/// after one gone before, and this tells them apart. On Linux that time is read as the Trash reads it now, in the
/// computer's time zone now ([`read_now`]). Never found there, at or after the moment it was sent ([`is_the_one`]).
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn went_in_then(item: &trash::TrashItem, r: &Removed) -> bool {
    let Some(at) = r.binned else { return item.time_deleted >= r.when.saturating_sub(2) };
    let then = r.binned_zone.map_or(at, |zone| read_now(at, zone));
    item.time_deleted.abs_diff(then) <= 2
}

/// How the Linux Trash reads now a time it read as `at` in the time zone `zone` (seconds ahead of UTC): it keeps the
/// clock's time without the zone, and reads that in the computer's zone now.
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn read_now(at: i64, zone: i32) -> i64 {
    use chrono::TimeZone;
    let clock = chrono::DateTime::from_timestamp(at.saturating_add(i64::from(zone)), 0).map(|t| t.naive_utc());
    clock.and_then(|clock| chrono::Local.from_local_datetime(&clock).earliest()).map_or(at, |t| t.timestamp())
}

/// The computer's time zone at a time the bin gave (seconds ahead of UTC), where the bin keeps times without it: on
/// Linux. Windows keeps them in UTC.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
fn zone_then(at: i64) -> Option<i32> {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(at, 0).earliest().map(|t| t.offset().local_minus_utc())
}

#[cfg(windows)]
fn zone_then(_at: i64) -> Option<i32> {
    None
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
    match mo_catch::catch(trash::os_limited::list) {
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
    let index = BinIndex::new(&items);
    let mut taken: HashSet<&std::ffi::OsStr> = HashSet::new();
    let mut problems = Vec::new();
    for r in removed.iter_mut() {
        // The first to go there after it was sent is this one.
        let found = index
            .near(r)
            .filter(|i| !taken.contains(i.id.as_os_str()) && is_the_one(i, r))
            .min_by_key(|i| i.time_deleted);
        match found {
            Some(item) => {
                taken.insert(item.id.as_os_str());
                r.bin_id = item.id.to_str().map(str::to_owned);
                r.binned = Some(item.time_deleted);
                r.binned_zone = zone_then(item.time_deleted);
            }
            None => problems.push(Problem { path: r.path.clone(), why: NOT_IN_BIN.into() }),
        }
    }
    problems
}

/// Puts back from the bin those of `removed` that `check` lets through (each looked at where the bin keeps it; all of
/// them when there's no check), and tells `each` how each went ([`Bin::put_back_checked`]). The bin is read once.
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn put_back_from_system_bin(
    removed: &[Removed],
    check: Option<&Check<'_>>,
    each: &mut dyn FnMut(usize, Option<String>),
) {
    let Some(items) = bin_items() else {
        for n in 0..removed.len() {
            each(n, Some(CANT_READ_BIN.to_owned()));
        }
        return;
    };
    // An item that's gone means it was put back already, or the bin was emptied: anything there now from its place
    // went later, so it isn't put back instead.
    for (n, (r, item)) in removed.iter().zip(matched(&items, removed)).enumerate() {
        let Some(item) = item else {
            each(n, Some(GONE_FROM_BIN.to_owned()));
            continue;
        };
        let checked = match check {
            None => Ok(()),
            Some(check) => in_bin_path(item).ok_or_else(|| CANT_READ_BIN.to_owned()).and_then(|at| check(r, &at)),
        };
        each(n, checked.and_then(|()| put_back_item(r, item).map_err(str::to_owned)).err());
    }
}

/// What Put back says of something no longer in the bin.
pub const GONE_FROM_BIN: &str = "It isn't in the Recycle Bin any more.";

/// What Put back says when the bin can't be read.
const CANT_READ_BIN: &str = "The Recycle Bin couldn't be read.";

/// What Put back says when something stays in the bin for no reason Windows gave.
const COULDNT_PUT_BACK: &str = "It couldn't be put back; it's still in the Recycle Bin.";

/// What Put back says when something new has the name of what it would put back.
pub const NAME_TAKEN: &str = "Something new has its name now, so it's left in the Recycle Bin.";

/// Puts one thing back from the bin item it is ([`matched`]), or says why it stays there.
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn put_back_item(r: &Removed, item: &trash::TrashItem) -> Result<(), &'static str> {
    // It comes back under its own full name, even when Windows listed it without its ending.
    let mut item = item.clone();
    if let Some(name) = Path::new(&r.real).file_name().filter(|_| same_name(&item, Path::new(&r.real))) {
        item.name = name.to_owned();
    }
    let spot = item.original_path();
    let was_there = fs::symlink_metadata(&spot).is_ok();
    let in_bin = in_bin_path(&item);
    match mo_catch::catch(AssertUnwindSafe(|| trash::os_limited::restore_all([item]))) {
        // Windows can say it's done when it stopped without saying why: what's there decides.
        Ok(Ok(())) if fs::symlink_metadata(&spot).is_ok() => Ok(()),
        Ok(Err(trash::Error::RestoreCollision { .. })) => Err(NAME_TAKEN),
        said => {
            // Only while it's still in the bin: one that came back (and only the bin's own note of it couldn't be
            // cleared) is the thing itself, however empty.
            if !was_there && in_bin.as_deref().is_some_and(|b| fs::symlink_metadata(b).is_ok()) {
                remove_placeholder(&spot);
            }
            Err(match said {
                Ok(Err(e)) if needs_admin_back(&e) => BACK_NEEDS_ADMIN,
                _ => COULDNT_PUT_BACK,
            })
        }
    }
}

/// Access denied, or it needs administrator rights, as Windows says it of a file of yours: its own word, and the copy
/// engine's (COPYENGINE_E_ACCESS_DENIED_SRC, COPYENGINE_E_REQUIRES_ELEVATION).
const NEEDS_RIGHTS: [u32; 3] = [0x8007_0005, 0x8027_0021, 0x8027_0002];

/// Whether Windows wouldn't put something back for want of administrator rights ([`NEEDS_RIGHTS`]), or wouldn't let
/// it into its place (COPYENGINE_E_ACCESS_DENIED_DEST: there, the place is where it goes back to, a folder Windows
/// guards; sending, it's your own Recycle Bin, where administrator rights wouldn't help).
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn needs_admin_back(e: &trash::Error) -> bool {
    const ACCESS_DENIED_DEST: u32 = 0x8027_0022;
    matches!(e, trash::Error::Os { code, .. }
        if NEEDS_RIGHTS.contains(&code.cast_unsigned()) || code.cast_unsigned() == ACCESS_DENIED_DEST)
}

/// On Linux, putting something back first makes an empty file or folder in its place, then moves it there from the
/// Trash: when that move fails (the Trash was on another drive), the empty one stays, and every later Undo would find
/// "something new" there. One that wasn't there before and is still empty is taken away again. Windows makes none.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
fn remove_placeholder(spot: &Path) {
    match fs::symlink_metadata(spot) {
        Ok(meta) if meta.is_file() && meta.len() == 0 => {
            let _ = fs::remove_file(spot);
        }
        // Only an empty folder can be removed this way.
        Ok(meta) if meta.is_dir() => {
            let _ = fs::remove_dir(spot);
        }
        _ => {}
    }
}

#[cfg(windows)]
fn remove_placeholder(_spot: &Path) {}

/// Where a bin item itself is kept. On Linux, from its note (`…/Trash/info/Name.trashinfo` keeps
/// `…/Trash/files/Name`), as the trash crate finds it; on Windows, it's what the bin calls it
/// (`C:\$Recycle.Bin\S-1-5-…\$R2X4KQ7.vst3`).
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
fn in_bin_path(item: &trash::TrashItem) -> Option<std::path::PathBuf> {
    let info = Path::new(&item.id);
    Some(info.parent()?.parent()?.join("files").join(info.file_stem()?))
}

#[cfg(windows)]
fn in_bin_path(item: &trash::TrashItem) -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(&item.id))
}

/// A Mac's Trash can't be read by a program: nothing to check.
#[cfg(not(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
fn find_in_system_bin(_removed: &mut [Removed]) -> Vec<Problem> {
    Vec::new()
}

/// Which of these are in the bin now ([`matched`]). `None` when the bin can't be read.
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn held_in_system_bin(removed: &[Removed]) -> Option<Vec<bool>> {
    bin_items().map(|items| held_among(&items, removed))
}

/// Which of `removed` are among the bin's `items` ([`matched`]).
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn held_among(items: &[trash::TrashItem], removed: &[Removed]) -> Vec<bool> {
    matched(items, removed).iter().map(Option::is_some).collect()
}

/// The bin item each of `removed` is, while it's still there: the one found when it went, by what the bin calls it,
/// while that's still from its place and went in when the bin said then ([`went_in_then`]: on Linux a name is used
/// again once its item has gone); one never found then, by its place and time, the first to go there after it was
/// sent. Each item is one thing's only, and the last sent is matched first: of two from the same place with the same
/// name, an item that went after both is the later one's.
#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn matched<'a>(items: &'a [trash::TrashItem], removed: &[Removed]) -> Vec<Option<&'a trash::TrashItem>> {
    let index = BinIndex::new(items);
    let mut found: Vec<Option<&trash::TrashItem>> = vec![None; removed.len()];
    let mut taken: HashSet<&std::ffi::OsStr> = HashSet::new();
    let mut last_first: Vec<usize> = (0..removed.len()).collect();
    last_first.sort_by_key(|&n| std::cmp::Reverse(removed[n].when));
    // Those found when they went first: what the bin calls them says which they are.
    for &n in &last_first {
        let r = &removed[n];
        let Some(id) = r.bin_id.as_deref().map(std::ffi::OsStr::new) else { continue };
        let item = index.called(id).find(|i| i.id.as_os_str() == id && from_there(i, r) && went_in_then(i, r));
        if let Some(item) = item
            && taken.insert(item.id.as_os_str())
        {
            found[n] = Some(item);
        }
    }
    for &n in &last_first {
        let r = &removed[n];
        if r.bin_id.is_some() {
            continue;
        }
        let first = index
            .near(r)
            .filter(|i| !taken.contains(i.id.as_os_str()) && is_the_one(i, r))
            .min_by_key(|i| i.time_deleted);
        if let Some(item) = first {
            taken.insert(item.id.as_os_str());
            found[n] = Some(item);
        }
    }
    found
}

/// A Mac's Trash can't be read by a program: what's in it can't be told.
#[cfg(not(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
fn held_in_system_bin(_removed: &[Removed]) -> Option<Vec<bool>> {
    None
}

/// A Mac's Trash can't be asked to put things back by a program: Finder's Put Back does it.
#[cfg(not(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
fn put_back_from_system_bin(
    removed: &[Removed],
    _check: Option<&Check<'_>>,
    each: &mut dyn FnMut(usize, Option<String>),
) {
    for n in 0..removed.len() {
        each(n, Some("Put it back from the Trash: select it there and choose Put Back.".to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::{Bin, IN_USE, KEPT_BY_YOU, NEEDS_ADMIN, SystemBin, bin_problem, on_network, text};

    #[test]
    fn a_heard_bin_tells_each_send_and_passes_every_way_on() {
        use super::{Admin, Check, Heard, Problem, Removed, What};
        use crate::plugin_space::SpaceKind;
        use std::path::Path;
        use std::sync::Mutex;

        struct Asks;
        impl Admin for Asks {
            fn can_ask(&self) -> bool {
                true
            }
            fn ask(&self, _: &[String]) -> Result<(), String> {
                Ok(())
            }
        }
        /// Answers each way unlike its default, and notes each it's asked: never the computer's bin.
        #[derive(Default)]
        struct Noting(Mutex<Vec<&'static str>>);
        impl Noting {
            fn note(&self, way: &'static str) {
                self.0.lock().unwrap().push(way);
            }
        }
        impl Bin for Noting {
            fn send(&self, path: &Path) -> Result<(), String> {
                self.note("send");
                if path.ends_with("in use") { Err("It's in use.".to_owned()) } else { Ok(()) }
            }
            fn admin(&self) -> &dyn Admin {
                self.note("admin");
                &Asks
            }
            fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
                self.note("find");
                removed[0].bin_id = Some("1".to_owned());
                vec![Problem { path: "a".to_owned(), why: "find".to_owned() }]
            }
            fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
                self.note("put_back");
                vec![Problem { path: "a".to_owned(), why: "put back".to_owned() }]
            }
            fn holds(&self, _: &[Removed]) -> Option<Vec<bool>> {
                self.note("holds");
                Some(vec![true])
            }
            fn put_back_checked(&self, _: &[Removed], _: &Check<'_>, each: &mut dyn FnMut(usize, Option<String>)) {
                self.note("put_back_checked");
                each(0, None);
            }
            fn can_look_for(&self, _: &Removed) -> bool {
                self.note("can_look_for");
                false
            }
            fn refuses_quietly(&self, _: &Path, _: u64) -> Option<String> {
                self.note("refuses_quietly");
                Some("quietly".to_owned())
            }
        }

        let inner = Noting::default();
        let heard = Mutex::new(Vec::new());
        let hear = |path: &Path, sent: &Result<(), String>| {
            heard.lock().unwrap().push((path.to_string_lossy().into_owned(), sent.clone()));
        };
        let bin = Heard::new(&inner, &hear);
        let one = Removed {
            path: "a".to_owned(),
            real: "a".to_owned(),
            bytes: 1,
            what: What::Plugin(SpaceKind::Sounds),
            when: 0,
            bin_id: None,
            binned: None,
            binned_zone: None,
            stamp: None,
        };
        assert_eq!(bin.send(Path::new("a")), Ok(()));
        assert_eq!(bin.send(Path::new("in use")), Err("It's in use.".to_owned()));
        assert_eq!(
            *heard.lock().unwrap(),
            [("a".to_owned(), Ok(())), ("in use".to_owned(), Err("It's in use.".to_owned()))]
        );
        assert!(bin.admin().can_ask());
        let mut found = vec![one.clone()];
        assert_eq!(bin.find(&mut found)[0].why, "find");
        assert_eq!(found[0].bin_id.as_deref(), Some("1"));
        assert_eq!(bin.put_back(std::slice::from_ref(&one))[0].why, "put back");
        assert_eq!(bin.holds(std::slice::from_ref(&one)), Some(vec![true]));
        let mut back = Vec::new();
        bin.put_back_checked(std::slice::from_ref(&one), &|_, _| Ok(()), &mut |n, why| back.push((n, why)));
        assert_eq!(back, [(0, None)]);
        assert!(!bin.can_look_for(&one));
        assert_eq!(bin.refuses_quietly(Path::new("a"), 1).as_deref(), Some("quietly"));
        // Each way reached the bin inside, none of them a default.
        assert_eq!(
            *inner.0.lock().unwrap(),
            [
                "send",
                "send",
                "admin",
                "find",
                "put_back",
                "holds",
                "put_back_checked",
                "can_look_for",
                "refuses_quietly"
            ]
        );
    }

    /// A failed Undo on Linux takes away the empty file or folder it made, and nothing else.
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    #[test]
    fn an_empty_placeholder_is_taken_away_and_nothing_else() {
        use super::remove_placeholder;
        use std::fs;
        let dir = std::env::temp_dir().join("mo-space-unit-placeholder");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (empty_file, empty_dir) = (dir.join("Song.bwproject"), dir.join("Folder"));
        let (full_file, full_dir) = (dir.join("Other.bwproject"), dir.join("Full"));
        fs::write(&empty_file, b"").unwrap();
        fs::create_dir(&empty_dir).unwrap();
        fs::write(&full_file, b"song").unwrap();
        fs::create_dir(&full_dir).unwrap();
        fs::write(full_dir.join("a.wav"), b"a").unwrap();
        for spot in [&empty_file, &empty_dir, &full_file, &full_dir, &dir.join("Never there")] {
            remove_placeholder(spot);
        }
        assert!(!empty_file.exists() && !empty_dir.exists());
        assert!(full_file.exists() && full_dir.join("a.wav").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_recycle_bin_takes_notes_quietly_only_when_its_on_and_has_room() {
        use super::{BIN_OFF, BinFacts, CANT_SEE_ROOM, DRIVE_FIXED, NO_ROOM, NOT_OWN_DRIVE, bin_decides};
        const GB: u64 = 1 << 30;
        let fixed = BinFacts {
            no_recycle_policy: false,
            kind: DRIVE_FIXED,
            nuke_on_delete: false,
            size: 1000 * GB,
            policy_percent: None,
            max_mb: None,
        };
        // No size set: a twentieth of the drive (50 GB), with a tenth to spare, so up to 45 GB.
        assert_eq!(bin_decides(&fixed, GB, || Some(44 * GB)), None);
        assert_eq!(bin_decides(&fixed, GB + 1, || Some(44 * GB)), Some(NO_ROOM));
        assert_eq!(bin_decides(&fixed, 1, || Some(u64::MAX)), Some(NO_ROOM));
        assert_eq!(bin_decides(&fixed, 1, || None), Some(CANT_SEE_ROOM));
        // The drive's own maximum size (1024 MB), so up to 0.9 GB.
        let own = BinFacts { max_mb: Some(1024), ..fixed };
        assert_eq!(bin_decides(&own, GB / 2, || Some(0)), None);
        assert_eq!(bin_decides(&own, 950_000_000, || Some(0)), None, "a MB is 1024 * 1024 bytes");
        assert_eq!(bin_decides(&own, GB, || Some(0)), Some(NO_ROOM));
        // A policy's share of the drive comes first (1%: 10 GB), and is never more than the whole drive.
        assert_eq!(bin_decides(&BinFacts { policy_percent: Some(1), ..own }, 8 * GB, || Some(0)), None);
        let all = BinFacts { policy_percent: Some(500), ..own };
        assert_eq!(bin_decides(&all, 900 * GB, || Some(0)), None);
        assert_eq!(bin_decides(&all, 901 * GB, || Some(0)), Some(NO_ROOM));
        // Off, or not one of the computer's own drives: never, however much room, and the bin isn't even read.
        let unread = || -> Option<u64> { panic!("not needed") };
        assert_eq!(bin_decides(&BinFacts { no_recycle_policy: true, ..fixed }, 1, unread), Some(BIN_OFF));
        assert_eq!(bin_decides(&BinFacts { nuke_on_delete: true, ..fixed }, 1, unread), Some(BIN_OFF));
        assert_eq!(bin_decides(&BinFacts { kind: 2, ..fixed }, 1, unread), Some(NOT_OWN_DRIVE));
    }

    /// The start-up sweep's notes stop going by themselves when Windows didn't move one and didn't say why, or was
    /// told no, or its Recycle Bin couldn't take it: what `trash` says then has to come out as [`KEPT_BY_YOU`]; a file
    /// in use, or one that needs administrator rights, is told apart by Windows' whole number.
    #[test]
    fn windows_not_saying_why_is_told_apart() {
        let os = |code: u32| bin_problem(&trash::Error::Os { code: code.cast_signed(), description: String::new() });
        // What `trash` 5.2.9 says then on Windows, whatever its words (elsewhere it means something else).
        let aborted = trash::Error::Unknown { description: "Some operations were aborted".to_owned() };
        let reworded = trash::Error::Unknown { description: "Not every file was moved".to_owned() };
        for unknown in [aborted, reworded] {
            assert_eq!(bin_problem(&unknown) == KEPT_BY_YOU, cfg!(windows), "{unknown:?}");
        }
        // Told no, or cancelled; or the Recycle Bin couldn't take it.
        for code in [0x8007_04C7, 0x8000_4004, 0x8027_0000, 0x8027_0001, 0x8027_0035, 0x8027_0036, 0x8027_003A] {
            assert_eq!(os(code), KEPT_BY_YOU, "{code:#x}");
        }
        for code in [0x8007_0020, 0x8007_0021, 0x8027_0027] {
            assert_eq!(os(code), IN_USE, "{code:#x}");
        }
        for code in [0x8007_0005, 0x8027_0021, 0x8027_0002] {
            assert_eq!(os(code), NEEDS_ADMIN, "{code:#x}");
        }
        // The same last digits from another part of Windows are something else, and so is "not implemented".
        for code in [0x8027_0005, 0x8027_0020, 0x8004_0005, 0x8007_0001, 0x8007_0078] {
            let said = os(code);
            assert!(said != KEPT_BY_YOU && said != IN_USE && said != NEEDS_ADMIN, "{code:#x}: {said}");
        }
    }

    /// Windows: a file a program holds without letting it be removed is told apart from one Windows was told no about,
    /// and so is one this account may not remove. Only opened to see: nothing is sent to the bin (rule 11).
    #[cfg(windows)]
    #[test]
    fn a_file_a_program_holds_is_told_apart() {
        use std::os::windows::fs::OpenOptionsExt;

        use super::{problem_with, why_kept};
        let dir = std::env::temp_dir().join(format!("mo-space-why-kept-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note-2026-07-01_10-00-00.png");
        std::fs::write(&file, b"x").unwrap();
        // Held as an image viewer may hold it: others may read it, but not remove it.
        let held = std::fs::OpenOptions::new().read(true).share_mode(0x1).open(&file).unwrap();
        assert_eq!(why_kept(&file), Some(IN_USE));
        // Only looked into when Windows didn't say why: a cancelled question stays a no, held or not.
        let unknown = trash::Error::Unknown { description: "Some operations were aborted".to_owned() };
        let cancelled = trash::Error::Os { code: 0x8007_04C7_u32.cast_signed(), description: String::new() };
        assert_eq!(problem_with(&unknown, &file), IN_USE);
        assert_eq!(problem_with(&cancelled, &file), KEPT_BY_YOU);
        drop(held);
        // Let go of: nothing tells why, so it counts as Windows having been told no.
        assert_eq!(why_kept(&file), None);
        assert!(file.exists(), "it was only looked at");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Windows: a folder tells too. One a program holds (as its working folder, say) is in use; one this account may
    /// not remove (as in Program Files) needs administrator rights, so it goes in the administrator step rather than
    /// staying as if Windows had been told no. The folder this account may not remove is made with icacls: removing
    /// it, and making things in the folder it's in, are denied to everyone, which holds for administrators too. Only
    /// opened to see: nothing is sent to the bin (rule 11).
    #[cfg(windows)]
    #[test]
    fn a_folder_a_program_holds_or_windows_guards_is_told_apart() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::path::Path;
        use std::process::Command;

        use super::{problem_with, why_kept};
        let dir = std::env::temp_dir().join(format!("mo-space-why-kept-folder-{}", std::process::id()));
        let held = dir.join("held (old copy)");
        std::fs::create_dir_all(&held).unwrap();
        std::fs::write(held.join("plugin.dll"), b"x").unwrap();
        let unknown = trash::Error::Unknown { description: "Some operations were aborted".to_owned() };
        // Nothing tells why: it counts as Windows having been told no, as before.
        assert_eq!(why_kept(&held), None);
        assert_eq!(problem_with(&unknown, &held), KEPT_BY_YOU);
        // Held as a program's working folder is: others may look in it, but not remove it.
        let holding =
            std::fs::OpenOptions::new().read(true).share_mode(0x1 | 0x2).custom_flags(0x0200_0000).open(&held).unwrap();
        assert_eq!(why_kept(&held), Some(IN_USE));
        assert_eq!(problem_with(&unknown, &held), IN_USE);
        drop(holding);
        // One this account may not remove: removing it is denied, and so is removing what's in the folder it's in.
        let guarded = dir.join("guarded (old copy)");
        std::fs::create_dir_all(&guarded).unwrap();
        let icacls = |path: &Path, args: &[&str]| {
            let ran = Command::new("icacls").arg(path).args(args).output().unwrap();
            assert!(ran.status.success(), "icacls {args:?}: {}", String::from_utf8_lossy(&ran.stdout));
        };
        // Takes the denies off again, even when the test stops partway, so the folders can be removed.
        struct Undeny<'a>([&'a Path; 2]);
        impl Drop for Undeny<'_> {
            fn drop(&mut self) {
                for path in self.0 {
                    let _ = Command::new("icacls").arg(path).args(["/remove:d", "*S-1-1-0"]).output();
                }
            }
        }
        let undeny = Undeny([&dir, &guarded]);
        // Everyone (S-1-1-0): DE removes it, DC removes what's in a folder.
        icacls(&guarded, &["/deny", "*S-1-1-0:(DE)"]);
        icacls(&dir, &["/deny", "*S-1-1-0:(DC)"]);
        let said = (why_kept(&guarded), problem_with(&unknown, &guarded));
        drop(undeny);
        assert_eq!(said, (Some(NEEDS_ADMIN), NEEDS_ADMIN.to_owned()));
        assert!(held.join("plugin.dll").exists() && guarded.exists(), "they were only looked at");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_server_path_is_on_the_network() {
        assert!(on_network(r"\\nas\music\Kontakt"));
        assert!(on_network("//nas/music/Kontakt"));
        // A folder that isn't there can't be asked about: only its path tells.
        assert!(!on_network("no such folder/Kontakt"));
    }

    #[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    #[test]
    fn what_is_still_in_the_bin_is_found_by_its_id_or_else_by_place_and_time() {
        use std::path::PathBuf;

        use super::{Removed, What, held_among};
        use crate::plugin_space::SpaceKind;

        let folder = PathBuf::from(if cfg!(windows) { r"C:\Presets" } else { "/presets" });
        let item = |id: &str, name: &str, at: i64| trash::TrashItem {
            id: id.into(),
            name: name.into(),
            original_parent: folder.clone(),
            time_deleted: at,
        };
        let removed = |name: &str, when: i64, bin_id: Option<&str>| {
            let path = folder.join(name).to_string_lossy().into_owned();
            let what = What::Plugin(SpaceKind::Presets);
            let bin_id = bin_id.map(str::to_owned);
            let binned = bin_id.as_ref().map(|_| when);
            Removed { real: path.clone(), path, bytes: 10, what, when, binned, binned_zone: None, bin_id, stamp: None }
        };
        let mut items = vec![item("1", "a.fxp", 100), item("2", "b.fxp", 200), item("3", "b.fxp", 300)];
        // What the bin called one of the app's is used again for something else from another folder (Linux does
        // that once the first is gone), and for a later thing of the same name from the same place.
        items.push(trash::TrashItem { original_parent: folder.join("Other"), ..item("4", "d.fxp", 400) });
        items.push(item("5", "e.fxp", 900));
        // Something of the same name from the same place went to the bin after both of these were sent.
        items.push(item("6", "f.fxp", 700));
        let asked = [
            // Found when it went, and still there.
            removed("a.fxp", 100, Some("1")),
            // Found when it went, but gone since: the bin was emptied, or it was put back.
            removed("c.fxp", 100, Some("9")),
            // Never found when it went: the first from its place since it went, then the next.
            removed("b.fxp", 150, None),
            removed("b.fxp", 250, None),
            // None left for it, and the one that would do is another's.
            removed("b.fxp", 250, None),
            removed("a.fxp", 50, None),
            // Its id is something else's now.
            removed("d.fxp", 350, Some("4")),
            // Its id is a later thing's now: the bin says that went in long after.
            removed("e.fxp", 500, Some("5")),
            // Of two never found, the later one's.
            removed("f.fxp", 600, None),
            removed("f.fxp", 650, None),
        ];
        let held = held_among(&items, &asked);
        assert_eq!(held, [true, false, true, true, false, false, false, false, false, true]);
        // Kept before the time was: found by what the bin calls it alone.
        let older = [Removed { binned: None, ..removed("e.fxp", 500, Some("5")) }];
        assert_eq!(held_among(&items, &older), [true]);

        // Linux keeps the time without the time zone. Found when the computer was an hour ahead of where it is now,
        // the bin reads that time an hour later now: that's when it went in, and nothing else is.
        use chrono::TimeZone;
        let day = 100_000;
        let zone = chrono::Local.timestamp_opt(day, 0).earliest().map_or(0, |t| t.offset().local_minus_utc());
        let in_bin = |at: i64| [item("20", "g.fxp", at)];
        let asked = |zone_then: i32| [Removed { binned_zone: Some(zone_then), ..removed("g.fxp", day, Some("20")) }];
        assert_eq!(held_among(&in_bin(day + 3600), &asked(zone + 3600)), [true]);
        assert_eq!(held_among(&in_bin(day + 3601), &asked(zone + 3600)), [true]);
        assert_eq!(held_among(&in_bin(day), &asked(zone + 3600)), [false]);
        // In the same zone, something of the same name from the same place binned an hour later, called the same once
        // the first had gone, is another thing.
        assert_eq!(held_among(&in_bin(day), &asked(zone)), [true]);
        assert_eq!(held_among(&in_bin(day + 3601), &asked(zone)), [false]);
    }

    /// The bin's items are looked through by where each came from: what's found is what looking through every item
    /// for each one found (as before), whatever the letter case, with Windows' names without their endings, two things
    /// from one place, and thousands of items; and fast.
    #[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
    #[test]
    fn what_is_in_the_bin_is_found_the_same_quickly_by_where_it_came_from() {
        use std::collections::HashSet;
        use std::path::PathBuf;

        use super::{BinIndex, Removed, What, from_there, held_among, is_the_one, went_in_then};
        use crate::song::CompactKind;

        let root = if cfg!(windows) { r"C:\Music" } else { "/music" };
        let folder = |f: usize| PathBuf::from(root).join(format!("Song {f}")).join("Backup");
        let item = |id: String, name: String, f: usize, at: i64| trash::TrashItem {
            id: id.into(),
            name: name.into(),
            original_parent: folder(f),
            time_deleted: at,
        };
        let mut items = Vec::new();
        for f in 0..300 {
            for n in 0..20 {
                let at = 1_000 + i64::try_from(f * 20 + n).unwrap_or_default();
                // Some named without their ending, as Windows can list them; some in capitals.
                let name = match n % 4 {
                    0 => format!("Song {f} (overwritten at {n:04})"),
                    1 => format!("SONG {f} (OVERWRITTEN AT {n:04}).FLP"),
                    _ => format!("Song {f} (overwritten at {n:04}).flp"),
                };
                items.push(item(format!("{f}-{n}.flp"), name, f, at));
            }
            // Two from one place.
            items.push(item(format!("{f}-again.flp"), format!("Song {f} (overwritten at 0002).flp"), f, 900_000));
        }
        let removed = |f: usize, n: usize, when: i64, bin_id: Option<String>| {
            let path = folder(f).join(format!("Song {f} (overwritten at {n:04}).flp")).to_string_lossy().into_owned();
            let binned = bin_id.as_ref().map(|_| 1_000 + i64::try_from(f * 20 + n).unwrap_or_default());
            let what = What::Song(CompactKind::Backup);
            Removed { real: path.clone(), path, bytes: 10, what, when, binned, binned_zone: None, bin_id, stamp: None }
        };
        let mut asked = Vec::new();
        for k in 0..3_000 {
            let (f, n) = (k * 7 % 300, k % 23);
            let bin_id = (k % 3 == 0).then(|| format!("{f}-{n}.flp"));
            asked.push(removed(f, n, 900 + i64::try_from(k % 5).unwrap_or_default() * 100, bin_id));
        }
        // What every item looked through for each one finds, as before the index.
        let slowly = |items: &[trash::TrashItem], removed: &[Removed]| -> Vec<bool> {
            let mut found = vec![false; removed.len()];
            let mut taken: HashSet<&std::ffi::OsStr> = HashSet::new();
            let mut last_first: Vec<usize> = (0..removed.len()).collect();
            last_first.sort_by_key(|&n| std::cmp::Reverse(removed[n].when));
            for &n in &last_first {
                let r = &removed[n];
                let Some(id) = r.bin_id.as_deref().map(std::ffi::OsStr::new) else { continue };
                let item = items.iter().find(|i| i.id.as_os_str() == id && from_there(i, r) && went_in_then(i, r));
                if let Some(item) = item
                    && taken.insert(item.id.as_os_str())
                {
                    found[n] = true;
                }
            }
            for &n in &last_first {
                let r = &removed[n];
                if r.bin_id.is_some() {
                    continue;
                }
                let first = items
                    .iter()
                    .filter(|i| !taken.contains(i.id.as_os_str()) && is_the_one(i, r))
                    .min_by_key(|i| i.time_deleted);
                if let Some(item) = first {
                    taken.insert(item.id.as_os_str());
                    found[n] = true;
                }
            }
            found
        };
        let some = &asked[..300];
        let held = held_among(&items, some);
        assert_eq!(held, slowly(&items, some));
        assert!(held.iter().any(|h| *h) && held.iter().any(|h| !*h));
        let at = std::time::Instant::now();
        assert_eq!(held_among(&items, &asked).len(), asked.len());
        let quick = at.elapsed();
        // Every item from a place is near what was there, whatever the case or ending, and nothing from elsewhere.
        let index = BinIndex::new(&items);
        let near: Vec<&trash::TrashItem> = index.near(&removed(5, 1, 0, None)).collect();
        assert_eq!(near.len(), 1);
        assert!(near.iter().all(|i| from_there(i, &removed(5, 1, 0, None))));
        assert_eq!(index.near(&removed(5, 2, 0, None)).count(), 2);
        assert_eq!(index.near(&removed(5, 0, 0, None)).count(), 1);
        assert_eq!(index.called(std::ffi::OsStr::new("5-2.flp")).count(), 1);
        // 3,000 things among 6,300 items: looked through item by item, that took seconds; with the index it takes
        // milliseconds, so this only fails if it's back to looking through every item (a busy computer has room).
        assert!(quick.as_secs() < 10, "{quick:?}");
    }

    /// Only asks: nothing is sent to the computer's bin here (rule 11).
    #[test]
    fn the_computers_bin_says_whether_it_takes_a_file_quietly() {
        let dir = std::env::temp_dir().join(format!("mo-space-quiet-bin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note-2026-07-01_10-00-00.md");
        std::fs::write(&file, b"x").unwrap();
        assert!(SystemBin.refuses_quietly(&dir.join("not there.md"), 1).is_some(), "what can't be looked at stays");
        let said = SystemBin.refuses_quietly(&file, 1);
        // Linux's Trash never asks (only a temp folder on a network drive stays); a Mac's Finder can, so nothing goes
        // there by itself. On Windows it depends on the computer's own Recycle Bin settings, but the drive and how
        // full its Recycle Bin is can always be read for the temp folder.
        if cfg!(target_os = "macos") {
            assert!(said.is_some());
        } else if cfg!(unix) {
            assert_eq!(said.is_some(), on_network(&text(&dir)), "{said:?}");
        }
        #[cfg(windows)]
        {
            let real = crate::real_path(&text(&file)).expect("it's there");
            let drive = super::Drive::of(std::path::Path::new(&real)).expect("Windows says which drive it's on");
            assert!(drive.id.starts_with('{') && drive.id.ends_with('}'), "{}", drive.id);
            assert_eq!(drive.kind, super::DRIVE_FIXED, "the temp folder is on one of the computer's own drives");
            assert!(drive.size > 0 && drive.bin_holds().is_some(), "how full its Recycle Bin is can be read");
            assert_ne!(said.as_deref(), Some(super::UNKNOWN_DRIVE));
            assert_ne!(said.as_deref(), Some(super::CANT_SEE_ROOM));
        }
        assert!(file.exists(), "it was only asked about");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_drive_that_isnt_plugged_in_cant_be_looked_in() {
        use super::{Removed, What};
        use crate::plugin_space::SpaceKind;
        let removed = |real: &std::path::Path| Removed {
            path: text(real),
            real: text(real),
            bytes: 1,
            what: What::Plugin(SpaceKind::Program),
            when: 1,
            bin_id: None,
            binned: None,
            binned_zone: None,
            stamp: None,
        };
        let dir = std::env::temp_dir();
        assert!(SystemBin.can_look_for(&removed(&dir.join("Serum.vst3"))));
        // A USB disk's folders are only there while it's plugged in; on Windows, its letter is.
        let gone = dir.join("mo-space-no-such-drive").join("Serum.vst3");
        assert_eq!(SystemBin.can_look_for(&removed(&gone)), cfg!(windows));
        assert!(!super::drive_is_here(""));
    }
}
