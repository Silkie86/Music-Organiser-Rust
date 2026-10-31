//! Moving a plugin's sounds or presets to another drive without the plugin noticing: the C# app's `FolderMove`
//! (Solomon, 26 Oct 2026: "a way to move some plugins data folders safely to a different drive").
//!
//! The folder is copied and every file checked, then the original is renamed to "… (old copy)" and a link (a junction
//! on Windows, which needs no administrator rights) takes its place, pointing at the new copy, so the plugin and every
//! project still find their files where they were. The old copy goes only when you say, after the plugin has opened
//! fine, only to the Recycle Bin, and only once nothing is only in it; [`move_back`] undoes the lot. Nothing here
//! deletes anything of yours: the only things ever removed for good are copies the move itself made, when it stops.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

use mo_formats::paths::file_name;
use mo_library::scan::is_inside;
use serde::{Deserialize, Serialize};

use crate::clean::{Bin, Done, Problem, What, on_network, send};
use crate::plugin_space::SpaceKind;
use crate::size::{self, is_link, is_online_only, size_text};
use crate::{key, real_path};

/// What the original is called while its link stands in for it.
pub const OLD_SUFFIX: &str = " (old copy)";

/// What a folder being copied back is called until it takes the link's place.
const BACK_SUFFIX: &str = " (moving back)";

/// What a folder is called for the moment it takes to see whether Windows lets it be renamed.
const CHECK_SUFFIX: &str = " (checking)";

/// What a file going in over an older one is called until it's whole.
const PART_SUFFIX: &str = ".mo-copying";

/// How far apart two times of the same file can be: some drives (exFAT, FAT32) keep times to two seconds.
const SAME_TIME: Duration = Duration::from_secs(2);

/// Room left over on the drive after the copy, so a move never fills it.
pub const HEADROOM: u64 = 2 << 30;

/// A plugin folder moved to another drive: where it was, where it is now, its old copy until you delete it, how big it
/// was and when it moved. Kept by the app as the C# app keeps it (the same JSON, so moves made by either are known to
/// both): `Kind` is the C# number (0 program, 1 presets, 2 sounds).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MovedFolder {
    pub from: String,
    pub to: String,
    pub old_copy: String,
    pub bytes: u64,
    /// When it moved, as the C# app writes it (`2026-10-26T14:03:12.1234567+01:00`).
    pub moved: String,
    /// The plugin it belongs to (its name key).
    #[serde(default)]
    pub key: String,
    #[serde(default = "sounds_number")]
    pub kind: u8,
    /// The other plugins whose folder it also was when it moved (a maker's folder).
    #[serde(default)]
    pub shared_with: Vec<String>,
}

fn sounds_number() -> u8 {
    2
}

impl MovedFolder {
    /// What it holds.
    pub fn space_kind(&self) -> SpaceKind {
        match self.kind {
            0 => SpaceKind::Program,
            1 => SpaceKind::Presets,
            _ => SpaceKind::Sounds,
        }
    }

    /// Whether the plugin with this name key had it: the one it was moved for, or one sharing it.
    pub fn belongs(&self, key: &str) -> bool {
        !key.is_empty() && (self.key == key || self.shared_with.iter().any(|k| k == key))
    }

    /// Whether the old copy is still beside the link, waiting for you to delete it.
    pub fn has_old_copy(&self) -> bool {
        fs::symlink_metadata(&self.old_copy).is_ok_and(|m| m.is_dir())
    }
}

/// The C# number for a kind.
fn kind_number(kind: SpaceKind) -> u8 {
    match kind {
        SpaceKind::Program => 0,
        SpaceKind::Presets => 1,
        SpaceKind::Sounds => 2,
    }
}

/// The moved folders kept as the C# app keeps them (library.db's `space_moved_folders` setting), or none when it can't
/// be read.
pub fn parse_moved(json: &str) -> Vec<MovedFolder> {
    serde_json::from_str(json).unwrap_or_default()
}

/// The moved folders as the C# app keeps them.
pub fn moved_json(moved: &[MovedFolder]) -> String {
    serde_json::to_string(moved).unwrap_or_else(|_| "[]".to_owned())
}

/// Which drive things are on and how much room it has. The app uses [`SystemDrives`]; tests stand in a second drive.
pub trait Drives: Sync {
    /// Whether two folders are on the same drive.
    fn same(&self, a: &Path, b: &Path) -> bool;
    /// How much room a drive has left, if it can be told.
    fn free(&self, path: &Path) -> Option<u64>;
}

/// The computer's own drives.
pub struct SystemDrives;

impl Drives for SystemDrives {
    fn same(&self, a: &Path, b: &Path) -> bool {
        same_drive(a, b)
    }
    fn free(&self, path: &Path) -> Option<u64> {
        fs4::available_space(path).ok()
    }
}

/// Unix: the same device. Windows: the same drive letter or network share, where the folders really are.
#[cfg(unix)]
fn same_drive(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev(),
        // Can't tell: say it's the same, so nothing moves.
        _ => true,
    }
}

#[cfg(not(unix))]
fn same_drive(a: &Path, b: &Path) -> bool {
    let root = |p: &Path| {
        let real = real_path(&p.to_string_lossy()).unwrap_or_else(|| p.to_string_lossy().into_owned());
        let k = key(&real);
        if let Some(share) = k.strip_prefix("//") {
            // //server/share
            let mut parts = share.splitn(3, '/');
            format!("//{}/{}", parts.next().unwrap_or_default(), parts.next().unwrap_or_default())
        } else {
            k.split('/').next().unwrap_or_default().to_owned()
        }
    };
    root(a) == root(b)
}

/// Where the folder goes in the folder you picked: a folder of the same name.
pub fn target(from: &str, destination: &str) -> PathBuf {
    Path::new(destination).join(file_name(from.trim_end_matches(['/', '\\'])))
}

/// The moved folder around a path (Native Instruments, moved, around Reaktor 6), if any: then the path isn't really on
/// this drive. Only the app's own moves count, and only while their link is there.
pub fn inside_moved<'a>(path: &str, moved: &'a [MovedFolder]) -> Option<&'a str> {
    moved
        .iter()
        .map(|m| m.from.as_str())
        .filter(|f| key(f) != key(path) && is_inside(path, f))
        .filter(|f| is_link(Path::new(f)))
        .min_by_key(|f| f.len())
}

/// Why a folder can't move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CantMove {
    /// It isn't there any more.
    Gone,
    /// It's a link already (moved, or set up that way).
    Link,
    /// It was moved before (to there), and a real folder is back where its link was.
    MovedBefore(String),
    /// It's inside a folder that's on another drive already (that moved folder).
    InsideMoved(String),
    /// There's a folder called "… (old copy)" beside it already.
    OldCopyThere,
    /// The folder picked isn't there.
    DestinationGone,
    /// The folder picked is inside the one being moved.
    DestinationInside,
    /// The folder picked is on the same drive.
    SameDrive,
    /// There's something of that name in the folder picked (its path).
    TargetThere(String),
    /// The folder picked is on a network drive.
    NetworkDrive,
    /// Some of its files are only online in OneDrive.
    OnlineOnly,
    /// The drive hasn't room: what it has free, and what's needed.
    NoRoom { free: u64, needs: u64 },
    /// Windows said no (access denied): something has the folder open, or it needs administrator rights.
    NeedsAdmin,
    /// Windows wouldn't rename it (something has it open): what it said.
    InUse(String),
    /// The move was stopped; nothing changed.
    Stopped,
    /// Something went wrong copying or swapping; nothing changed: what.
    Failed(String),
    /// Something went wrong part way and things aren't as they were: what to do about it.
    LeftHalfway(String),
}

impl CantMove {
    /// What the app says.
    pub fn text(&self) -> String {
        match self {
            CantMove::Gone => "The folder isn't there any more.".to_owned(),
            CantMove::Link => "It's moved already: it points to another place.".to_owned(),
            CantMove::MovedBefore(to) => format!(
                "It was moved to {to} before, and a real folder is back where its link was (an update may have put it \
                 there). Linking it again comes in a later version. Nothing was moved."
            ),
            CantMove::InsideMoved(outer) => format!(
                "It's inside {}, which is on another drive already, so moving it frees nothing here.",
                file_name(outer)
            ),
            CantMove::OldCopyThere => "There's already a folder called … (old copy) beside it.".to_owned(),
            CantMove::DestinationGone => "The folder you picked isn't there.".to_owned(),
            CantMove::DestinationInside => "That folder is inside the one being moved.".to_owned(),
            CantMove::SameDrive => {
                "That's the same drive, so moving there frees nothing. Pick a folder on another drive.".to_owned()
            }
            CantMove::TargetThere(t) => format!("{t} is there already. Pick another folder, or rename that one."),
            CantMove::NetworkDrive => {
                "That's a network drive: whenever it isn't there, the plugin couldn't find its files. Pick a drive \
                 in this computer."
                    .to_owned()
            }
            CantMove::OnlineOnly => "Some of its files are only online in OneDrive, so copying them would download \
                                     them onto this drive first. In File Explorer, right-click the folder and pick \
                                     Always keep on this device. Once they've downloaded, try again."
                .to_owned(),
            CantMove::NoRoom { free, needs } => format!(
                "That drive has {} free; this needs {} and some room to spare.",
                size_text(*free),
                size_text(*needs)
            ),
            CantMove::NeedsAdmin => "Windows said no. Close your DAW, the plugin and any window showing the folder, \
                                     then try again. If Windows still says no, the folder needs administrator rights, \
                                     which this version can't ask for yet. Nothing was moved."
                .to_owned(),
            CantMove::InUse(why) => format!(
                "Windows wouldn't rename the folder ({why}). Close your DAW and the plugin, then try again. Nothing \
                 was moved."
            ),
            CantMove::Stopped => "Stopped: nothing was moved.".to_owned(),
            CantMove::Failed(why) => format!("{why} Nothing was moved."),
            CantMove::LeftHalfway(what) => what.clone(),
        }
    }
}

/// Why the folder can't move to `destination`, or `None` when it can. `bytes` is its size; `moved` is the folders moved
/// already. Reads the folder to look for OneDrive files that are only online: call it away from the window's thread.
pub fn problem(
    from: &str,
    destination: &str,
    bytes: u64,
    drives: &dyn Drives,
    moved: &[MovedFolder],
    stop: &AtomicBool,
) -> Option<CantMove> {
    let from = from.trim_end_matches(['/', '\\']);
    let from_path = Path::new(from);
    if is_link(from_path) {
        return Some(CantMove::Link);
    }
    if !fs::symlink_metadata(from_path).is_ok_and(|m| m.is_dir()) {
        return Some(CantMove::Gone);
    }
    // Moving it again would make a second copy and lose track of the first, with what was saved through its link.
    if let Some(before) = moved.iter().find(|m| key(&m.from) == key(from)) {
        return Some(CantMove::MovedBefore(before.to.clone()));
    }
    if let Some(outer) = inside_moved(from, moved) {
        return Some(CantMove::InsideMoved(outer.to_owned()));
    }
    if fs::symlink_metadata(format!("{from}{OLD_SUFFIX}")).is_ok() {
        return Some(CantMove::OldCopyThere);
    }
    let real_from = real_path(from).unwrap_or_else(|| from.to_owned());
    let real_destination = real_path(destination).unwrap_or_else(|| destination.to_owned());
    if is_inside(destination, from) || is_inside(&real_destination, &real_from) {
        return Some(CantMove::DestinationInside);
    }
    if on_network(destination) || on_network(&real_destination) {
        return Some(CantMove::NetworkDrive);
    }
    if !fs::metadata(destination).is_ok_and(|m| m.is_dir()) {
        return Some(CantMove::DestinationGone);
    }
    if drives.same(from_path, Path::new(destination)) {
        return Some(CantMove::SameDrive);
    }
    let to = target(from, destination);
    if fs::symlink_metadata(&to).is_ok() {
        return Some(CantMove::TargetThere(to.to_string_lossy().into_owned()));
    }
    // Copying a OneDrive file that's only online downloads it first, onto the drive being freed.
    match size::has_online_only(from_path, stop) {
        Ok(true) => return Some(CantMove::OnlineOnly),
        Ok(false) => {}
        Err(_) => return Some(CantMove::Stopped),
    }
    let needs = bytes.saturating_add(HEADROOM);
    if let Some(free) = drives.free(Path::new(destination)).filter(|&free| free < needs) {
        return Some(CantMove::NoRoom { free, needs: bytes });
    }
    None
}

/// Copies the folder to the other drive and checks every file arrived whole, then swaps the original for a link to the
/// copy. If anything goes wrong before the swap, the half-made copy is removed and nothing else has changed. `kind` and
/// `key` say whose folder it is; `shared_with`, the other plugins it's also the folder of. `progress` hears the bytes
/// copied so far. Call it away from the window's thread.
#[allow(clippy::too_many_arguments)]
pub fn move_folder(
    from: &str,
    destination: &str,
    bytes: u64,
    (key_of, kind, shared_with): (&str, SpaceKind, &[String]),
    drives: &dyn Drives,
    already_moved: &[MovedFolder],
    progress: &(dyn Fn(u64) + Sync),
    stop: &AtomicBool,
) -> Result<MovedFolder, CantMove> {
    let from = from.trim_end_matches(['/', '\\']).to_owned();
    if let Some(why) = problem(&from, destination, bytes, drives, already_moved, stop) {
        return Err(why);
    }
    let to = target(&from, destination);
    // Find out before copying whether Windows lets the folder be renamed, rather than after an hour of copying.
    check_rename(&from)?;
    // The old copies of folders moved from inside it before (Reaktor 6's inside Native Instruments) aren't copied:
    // they stay in its own old copy, which can't go while their links don't reach their copies.
    let leave_out: Vec<String> =
        already_moved.iter().filter(|m| is_inside(&m.old_copy, &from)).map(|m| key(&m.old_copy)).collect();
    // Made here, so a folder that appeared meanwhile is never copied into (or later removed).
    fs::create_dir(&to).map_err(|e| CantMove::Failed(format!("Couldn't make {} ({e}).", to.to_string_lossy())))?;
    let source = Path::new(&from);
    let copied = copy_tree(source, &to, Fill::New, &to, &leave_out, progress, stop).and_then(|first| {
        // What was saved in the folder while it copied (an hour, for a big one) goes across too.
        copy_tree(source, &to, Fill::Freshen, &to, &leave_out, &|n| progress(first + n), stop)
    });
    if let Err(why) = copied {
        remove_copy(&to);
        return Err(why);
    }
    let aside = format!("{from}{OLD_SUFFIX}");
    if let Err(why) = swap(&from, &to, &aside) {
        remove_copy(&to);
        return Err(why);
    }
    Ok(MovedFolder {
        from,
        to: to.to_string_lossy().into_owned(),
        old_copy: aside,
        bytes,
        moved: csharp_time(chrono::Local::now()),
        key: key_of.to_owned(),
        kind: kind_number(kind),
        shared_with: shared_with.to_vec(),
    })
}

/// A time as the C# app writes one: seven digits after the second (chrono's own `%.Nf` only does 3, 6 or 9).
fn csharp_time(now: chrono::DateTime<chrono::Local>) -> String {
    use chrono::Timelike;
    let ticks = (now.nanosecond() / 100).min(9_999_999);
    format!("{}.{ticks:07}{}", now.format("%Y-%m-%dT%H:%M:%S"), now.format("%:z"))
}

/// Undoes a move: the link goes and the old copy takes its place again, with what changed on the other drive since the
/// move copied into it first (presets the plugin wrote through the link live only there). Once the old copy is deleted,
/// the folder is copied back instead, if this drive has room. The copy on the other drive is left for you to remove.
/// Only while the link still leads to the copy the move made.
pub fn move_back(
    moved: &MovedFolder,
    drives: &dyn Drives,
    progress: &(dyn Fn(u64) + Sync),
    stop: &AtomicBool,
) -> Result<(), CantMove> {
    let from = moved.from.as_str();
    let to = Path::new(&moved.to);
    if !is_link(Path::new(from)) {
        return Err(CantMove::Failed(format!("{from} isn't the link the move left, so it was left alone.")));
    }
    if !fs::metadata(to).is_ok_and(|m| m.is_dir()) {
        return Err(CantMove::Failed(format!("{} isn't there.", moved.to)));
    }
    if !link_reaches(moved) {
        return Err(CantMove::Failed(format!(
            "{from} leads somewhere else now, not to {}, so it was left alone.",
            moved.to
        )));
    }
    let settled = Path::new(from);
    if moved.has_old_copy() {
        // New files, and files that differ, go into the old copy; nothing in it is removed.
        copy_tree(to, Path::new(&moved.old_copy), Fill::Freshen, settled, &[], progress, stop)?;
        return unswap(from, &moved.old_copy, to);
    }
    let back = format!("{from}{BACK_SUFFIX}");
    if fs::symlink_metadata(&back).is_ok() {
        return Err(CantMove::Failed(format!(
            "There's a folder called {} beside it, left from an earlier try: look in it, then delete or rename it.",
            file_name(&back)
        )));
    }
    // The whole folder comes back onto the drive the move freed: only with room to spare.
    let needs = size::measure(to, stop).map_err(|_| CantMove::Stopped)?.bytes;
    let here = settled.parent().unwrap_or(settled);
    if let Some(free) = drives.free(here).filter(|&free| free < needs.saturating_add(HEADROOM)) {
        return Err(CantMove::NoRoom { free, needs });
    }
    fs::create_dir(&back).map_err(|e| CantMove::Failed(format!("Couldn't make {back} ({e}).")))?;
    if let Err(why) = copy_tree(to, Path::new(&back), Fill::New, settled, &[], progress, stop) {
        remove_copy(Path::new(&back));
        return Err(why);
    }
    let swapped = unswap(from, &back, to);
    if swapped.is_err() && link_reaches(moved) {
        // As it was: the plugin still uses the other drive, so the copy made to come back goes.
        remove_copy(Path::new(&back));
    }
    swapped
}

/// Whether the link where the folder was leads to the copy on the other drive, and that copy is there.
fn link_reaches(moved: &MovedFolder) -> bool {
    is_link(Path::new(&moved.from)) && leads_to(&moved.from, Path::new(&moved.to))
}

/// Whether a path ends up at a folder that's there, following any links on the way.
fn leads_to(path: &str, to: &Path) -> bool {
    match (real_path(path), real_path(&to.to_string_lossy())) {
        (Some(a), Some(b)) => key(&a) == key(&b) && fs::metadata(to).is_ok_and(|m| m.is_dir()),
        _ => false,
    }
}

/// Whether the old copy beside the link can go: the link is there and leads to the copy on the other drive, and that
/// copy is there. Not when the drive is unplugged or the copy was moved or renamed (then the old copy is the only one
/// the plugin could use), nor when an update put a real folder back where the link was.
pub fn old_copy_can_go(moved: &MovedFolder) -> bool {
    moved.has_old_copy() && link_reaches(moved)
}

/// The folders moved from inside this one before it moved, whose old copies sit in its old copy now.
fn inner_moves<'a>(moved: &'a MovedFolder, all: &'a [MovedFolder]) -> impl Iterator<Item = &'a MovedFolder> {
    all.iter().filter(move |m| key(&m.from) != key(&moved.from) && is_inside(&m.from, &moved.from))
}

/// Where a path inside the moved folder, as it was, sits in its old copy now.
fn in_old_copy(path: &str, moved: &MovedFolder) -> Option<PathBuf> {
    if !is_inside(path, &moved.from) {
        return None;
    }
    let rest = path.get(moved.from.trim_end_matches(['/', '\\']).len()..)?.trim_start_matches(['/', '\\']);
    Some(Path::new(&moved.old_copy).join(rest))
}

/// What's in the old copy but not in the copy on the other drive, or newer in the old copy, as paths inside it: files
/// saved there while the move ran, or deleted from the other drive since. The old copies of folders moved from inside
/// it before are left out (their own links are checked instead). Reads both folders: call it away from the window's
/// thread.
pub fn only_in_old_copy(moved: &MovedFolder, all: &[MovedFolder], stop: &AtomicBool) -> Result<Vec<String>, CantMove> {
    let old = PathBuf::from(&moved.old_copy);
    let skip: Vec<String> = inner_moves(moved, all)
        .filter_map(|m| in_old_copy(&m.old_copy, moved))
        .map(|p| key(&p.to_string_lossy()))
        .collect();
    let shown = |p: &Path| match p.strip_prefix(&old) {
        Ok(rest) if !rest.as_os_str().is_empty() => rest.to_string_lossy().into_owned(),
        _ => p.to_string_lossy().into_owned(),
    };
    let mut only = Vec::new();
    let mut folders = vec![(old.clone(), PathBuf::from(&moved.to))];
    while let Some((old_dir, copy_dir)) = folders.pop() {
        let Ok(entries) = fs::read_dir(&old_dir) else {
            only.push(shown(&old_dir));
            continue;
        };
        for entry in entries {
            if stop.load(Ordering::Relaxed) {
                return Err(CantMove::Stopped);
            }
            let Ok(entry) = entry else {
                only.push(shown(&old_dir));
                continue;
            };
            let path = entry.path();
            if skip.contains(&key(&path.to_string_lossy())) {
                continue;
            }
            let there = copy_dir.join(entry.file_name());
            let Ok(kind) = entry.file_type() else {
                only.push(shown(&path));
                continue;
            };
            let in_copy = if kind.is_symlink() {
                fs::symlink_metadata(&there).is_ok()
            } else if kind.is_dir() {
                let is_dir = fs::metadata(&there).is_ok_and(|m| m.is_dir());
                if is_dir {
                    folders.push((path.clone(), there));
                }
                is_dir
            } else {
                match (entry.metadata(), fs::metadata(&there)) {
                    (Ok(mine), Ok(copy)) => copy.is_file() && (same_file(&mine, &copy) || newer(&copy, &mine)),
                    _ => false,
                }
            };
            if !in_copy {
                only.push(shown(&path));
            }
        }
    }
    only.sort();
    Ok(only)
}

/// Sends the old copy to the Recycle Bin, once [`old_copy_can_go`] says it may, and the old copies of folders moved
/// from inside it before can go too (their links reach their copies). Unless you're `sure`, also only when nothing is
/// only in the old copy ([`only_in_old_copy`]): the app shows that list first. `all` is every moved folder.
/// [`crate::clean::undo`] puts it back.
pub fn remove_old_copy(moved: &MovedFolder, all: &[MovedFolder], sure: bool, bin: &dyn Bin, stop: &AtomicBool) -> Done {
    if !old_copy_can_go(moved) {
        return stays(
            moved,
            "It's the only copy the plugin could use now (the link or the copy on the other drive isn't as the move \
             left it), so it stays.",
        );
    }
    let held = inner_moves(moved, all)
        .filter(|m| in_old_copy(&m.old_copy, moved).is_some_and(|p| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())));
    if let Some(inner) = held.into_iter().find(|m| !link_reaches(m)) {
        return stays(
            moved,
            &format!(
                "It holds the old copy of {}, whose link doesn't lead to {} now, so it stays.",
                file_name(&inner.from),
                inner.to
            ),
        );
    }
    if !sure {
        match only_in_old_copy(moved, all, stop) {
            Err(_) => return stays(moved, "Stopped before it was all checked, so it stays."),
            Ok(only) if !only.is_empty() => {
                let first: Vec<&str> = only.iter().take(5).map(String::as_str).collect();
                let more = if only.len() > first.len() {
                    format!(" and {} more", only.len() - first.len())
                } else {
                    String::new()
                };
                return stays(
                    moved,
                    &format!(
                        "Some of it is only in the old copy, or newer there: {}{more}. It may have been saved while the \
                         move ran, or deleted from the other drive since. Look before deleting it.",
                        first.join(", ")
                    ),
                );
            }
            Ok(_) => {}
        }
    }
    let mut done = Done::default();
    let real = real_path(&moved.old_copy).unwrap_or_else(|| moved.old_copy.clone());
    send(&moved.old_copy, real, moved.bytes, What::Plugin(moved.space_kind()), bin, &mut done);
    let not_found = bin.find(&mut done.removed);
    done.problems.extend(not_found);
    done
}

/// The old copy stays, and why.
fn stays(moved: &MovedFolder, why: &str) -> Done {
    Done { problems: vec![Problem { path: moved.old_copy.clone(), why: why.to_owned() }], ..Done::default() }
}

/// Whether Windows said no because of rights (access denied), rather than something having the folder open.
fn denied(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied
}

/// Renames the folder and straight back, to learn whether the swap will work.
fn check_rename(from: &str) -> Result<(), CantMove> {
    let check = format!("{from}{CHECK_SUFFIX}");
    if fs::symlink_metadata(&check).is_ok() {
        return Err(CantMove::Failed(format!("There's a folder called {} beside it.", file_name(&check))));
    }
    match fs::rename(from, &check) {
        Ok(()) => {}
        Err(e) if denied(&e) => return Err(CantMove::NeedsAdmin),
        Err(e) => return Err(CantMove::InUse(e.to_string())),
    }
    rename_back(&check, from).map_err(|e| {
        CantMove::LeftHalfway(format!(
            "Windows let the folder be renamed but not renamed back ({e}), so it's called \"{}\" for now. Rename it \
             to \"{}\" yourself. Nothing was copied.",
            file_name(&check),
            file_name(from)
        ))
    })
}

/// Renames a folder, trying again for a few seconds while something (a virus checker) looks in.
fn rename_back(from: &str, to: &str) -> io::Result<()> {
    let mut tries = 0;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(_) if tries < 20 && fs::symlink_metadata(to).is_err() => {
                tries += 1;
                thread::sleep(Duration::from_millis(250));
            }
            Err(e) => return Err(e),
        }
    }
}

/// The original becomes "… (old copy)" and a link to the copy takes its name. If the link doesn't take, the original
/// gets its name back.
fn swap(from: &str, to: &Path, aside: &str) -> Result<(), CantMove> {
    if fs::symlink_metadata(aside).is_ok() {
        return Err(CantMove::OldCopyThere);
    }
    match fs::rename(from, aside) {
        Ok(()) => {}
        Err(e) if denied(&e) => return Err(CantMove::NeedsAdmin),
        Err(e) => return Err(CantMove::InUse(e.to_string())),
    }
    let made = link(Path::new(from), to);
    if made.is_ok() && leads_to(from, to) {
        return Ok(());
    }
    // Nothing else can be where the folder was: what's there is what the link left.
    clear_failed_link(Path::new(from));
    match (made, rename_back(aside, from)) {
        (_, Err(e)) => Err(CantMove::LeftHalfway(format!(
            "The link couldn't be made, and Windows wouldn't give the folder its name back ({e}), so it's called \
             \"{}\" for now. Rename it to \"{}\" yourself: everything is in it.",
            file_name(aside),
            file_name(from)
        ))),
        (Err(e), Ok(())) if denied(&e) => Err(CantMove::NeedsAdmin),
        (Err(e), Ok(())) => Err(CantMove::Failed(format!("Windows wouldn't make the link ({e})."))),
        (Ok(()), Ok(())) => Err(CantMove::Failed("The link was made but doesn't reach the copy.".to_owned())),
    }
}

/// The link goes and `restore` takes its name. If that fails, the link to `to` goes back; if even that fails, says
/// what to rename.
fn unswap(from: &str, restore: &str, to: &Path) -> Result<(), CantMove> {
    match remove_link(Path::new(from)) {
        Ok(()) => {}
        Err(e) if denied(&e) => return Err(CantMove::NeedsAdmin),
        Err(e) => return Err(CantMove::InUse(e.to_string())),
    }
    let Err(e) = rename_back(restore, from) else { return Ok(()) };
    if fs::symlink_metadata(from).is_err() {
        // The link goes back, so the plugin still finds its files on the other drive.
        if link(Path::new(from), to).is_ok() {
            return Err(if denied(&e) { CantMove::NeedsAdmin } else { CantMove::InUse(e.to_string()) });
        }
        clear_failed_link(Path::new(from));
    }
    Err(CantMove::LeftHalfway(format!(
        "The link was taken away, but Windows wouldn't rename \"{}\" to \"{}\" ({e}), nor put the link back. Rename it \
         yourself: everything is in it, and the plugin looks for it there.",
        file_name(restore),
        file_name(from)
    )))
}

/// A junction on Windows (no administrator rights or developer mode needed), a symbolic link elsewhere.
fn link(at: &Path, target: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        junction::create(target, at)
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, at)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (at, target);
        Err(io::Error::new(io::ErrorKind::Unsupported, "no links here"))
    }
}

/// A link to a file (on Windows, only an administrator or developer mode may make one).
fn file_link(target: &Path, at: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, at)
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, at)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (at, target);
        Err(io::Error::new(io::ErrorKind::Unsupported, "no links here"))
    }
}

/// Clears what a link that didn't take left: the link, or the empty folder Windows makes a junction from. Only ever an
/// empty folder: one with anything in it stays.
fn clear_failed_link(at: &Path) {
    if is_link(at) {
        let _ = remove_link(at);
    } else if fs::symlink_metadata(at).is_ok_and(|m| m.is_dir()) {
        let _ = fs::remove_dir(at);
    }
}

/// Removes a link only, never what it leads to.
fn remove_link(at: &Path) -> io::Result<()> {
    if !is_link(at) {
        return Err(io::Error::other("not a link"));
    }
    // A junction or a folder link is removed as a folder on Windows; a symbolic link as a file elsewhere.
    if cfg!(windows) { fs::remove_dir(at) } else { fs::remove_file(at) }
}

/// Removes a copy the move made itself, when the move stops: never a link, never the original.
fn remove_copy(path: &Path) {
    if fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) && !is_link(path) {
        let _ = fs::remove_dir_all(path);
    }
}

/// How [`copy_tree`] treats what's in the copy already.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fill {
    /// The copy was just made, so anything there already is a second file of the same name but for capitals, on a
    /// drive that doesn't tell them apart: the copy stops.
    New,
    /// The copy has most of it: files that are new, or differ in size or time, are copied; nothing is removed.
    Freshen,
}

/// Copies a folder into another one, file by file, keeping each file's time and checking its size once written, and
/// returns the bytes copied. A link inside is made again as a link, never followed: one leading inside the folder
/// leads to the same place inside `settled`, where the copy ends up. Folders in `leave_out` (keys) aren't copied. One
/// file at a time: a move usually goes to a spinning drive, where several at once is slower.
fn copy_tree(
    from: &Path,
    to: &Path,
    fill: Fill,
    settled: &Path,
    leave_out: &[String],
    progress: &(dyn Fn(u64) + Sync),
    stop: &AtomicBool,
) -> Result<u64, CantMove> {
    let source_real = fs::canonicalize(from).map_err(|e| failed(from, &e))?;
    let mut done = 0u64;
    let mut folders = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((source_dir, copy_dir)) = folders.pop() {
        let entries = fs::read_dir(&source_dir).map_err(|e| failed(&source_dir, &e))?;
        for entry in entries {
            if stop.load(Ordering::Relaxed) {
                return Err(CantMove::Stopped);
            }
            let entry = entry.map_err(|e| failed(&source_dir, &e))?;
            let source = entry.path();
            if leave_out.contains(&key(&source.to_string_lossy())) {
                continue;
            }
            let copy = copy_dir.join(entry.file_name());
            let there = fs::symlink_metadata(&copy).ok();
            if fill == Fill::New && there.is_some() {
                return Err(CantMove::Failed(format!(
                    "{} has a twin whose name differs only in capitals, and the drive picked doesn't tell them apart.",
                    source.to_string_lossy()
                )));
            }
            let kind = entry.file_type().map_err(|e| failed(&source, &e))?;
            if kind.is_symlink() {
                // Freshening: a link there already stays as it is.
                if there.is_none() {
                    copy_link(&source, &kind, &copy, &source_real, settled)?;
                }
                continue;
            }
            if kind.is_dir() {
                match &there {
                    None => fs::create_dir(&copy).map_err(|e| failed(&copy, &e))?,
                    Some(m) if m.is_dir() => {}
                    // Never written through a link: it could lead into another plugin's folder.
                    Some(_) => {
                        return Err(CantMove::Failed(format!(
                            "{} is a file or a link in the copy, so the folder of that name couldn't go there.",
                            copy.to_string_lossy()
                        )));
                    }
                }
                folders.push((source, copy));
                continue;
            }
            let meta = entry.metadata().map_err(|e| failed(&source, &e))?;
            // Never downloaded to be copied (one that went online-only after the check): it stops before the swap.
            if is_online_only(&meta) {
                return Err(CantMove::OnlineOnly);
            }
            if let Some(m) = &there {
                if !m.is_file() {
                    return Err(CantMove::Failed(format!(
                        "{} is a folder or a link in the copy, so the file of that name couldn't go there.",
                        copy.to_string_lossy()
                    )));
                }
                if same_file(m, &meta) {
                    continue;
                }
            }
            copy_file(&source, &copy, &meta, there.is_some())?;
            done += meta.len();
            progress(done);
        }
    }
    Ok(done)
}

/// What a copy that went wrong says.
fn failed(path: &Path, e: &io::Error) -> CantMove {
    CantMove::Failed(format!("Copying stopped at {} ({e}).", path.to_string_lossy()))
}

/// Copies one file, keeps its time and checks it arrived whole. Over a file that's there already it's written under a
/// temporary name first and takes that file's place only once whole, so one that fails leaves the old file as it was.
fn copy_file(source: &Path, copy: &Path, meta: &fs::Metadata, replace: bool) -> Result<(), CantMove> {
    let when = meta.modified().map_err(|e| failed(source, &e))?;
    let write_to = if replace {
        let mut name = copy.file_name().unwrap_or_default().to_os_string();
        name.push(PART_SUFFIX);
        copy.with_file_name(name)
    } else {
        copy.to_path_buf()
    };
    if replace && fs::symlink_metadata(&write_to).is_ok_and(|m| !m.is_file()) {
        return Err(CantMove::Failed(format!("{} is in the way.", write_to.to_string_lossy())));
    }
    let written = fs::copy(source, &write_to)
        .and_then(|_| keep_time(&write_to, when))
        .and_then(|()| fs::metadata(&write_to))
        .map_err(|e| failed(source, &e))
        .and_then(|written| {
            if written.len() == meta.len() {
                Ok(())
            } else {
                Err(CantMove::Failed(format!("{} didn't copy whole.", source.to_string_lossy())))
            }
        })
        .and_then(|()| if replace { fs::rename(&write_to, copy).map_err(|e| failed(copy, &e)) } else { Ok(()) });
    if written.is_err() && replace {
        let _ = fs::remove_file(&write_to);
    }
    written
}

/// Whether two files look the same: the same size, and times no further apart than a drive's clock steps.
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.len() == b.len()
        && match (a.modified(), b.modified()) {
            (Ok(x), Ok(y)) => x.duration_since(y).unwrap_or_else(|e| e.duration()) < SAME_TIME,
            _ => false,
        }
}

/// Whether `a` was changed later than `b`, by more than a drive's clock steps.
fn newer(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    matches!((a.modified(), b.modified()), (Ok(x), Ok(y)) if x.duration_since(y).is_ok_and(|d| d >= SAME_TIME))
}

/// Gives a copied file the original's time. Opened only to change its times, so a read-only file (a library's own
/// samples often are) works too.
fn keep_time(copy: &Path, when: SystemTime) -> io::Result<()> {
    let mut options = fs::File::options();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        /// Windows' FILE_WRITE_ATTRIBUTES: enough to change a file's times, even a read-only one's.
        const WRITE_ATTRIBUTES: u32 = 0x100;
        options.access_mode(WRITE_ATTRIBUTES);
    }
    #[cfg(not(windows))]
    options.read(true);
    options.open(copy)?.set_modified(when)
}

/// Makes a link in the copy leading to the real place the original link leads to, by its whole path: a link worded
/// from where it sits ("..\Shared") would lead somewhere else from the other drive. One leading inside the folder being
/// copied leads to the same place inside `settled` instead. One that leads nowhere now (a drive that isn't plugged in)
/// is made again as it was worded. A file link Windows won't let be made again is copied as the file it leads to.
fn copy_link(
    link_path: &Path,
    kind: &fs::FileType,
    copy: &Path,
    source_real: &Path,
    settled: &Path,
) -> Result<(), CantMove> {
    let couldnt = |e: io::Error| {
        CantMove::Failed(format!("{} is a link that couldn't be made again ({e}).", link_path.to_string_lossy()))
    };
    let Ok(real) = fs::canonicalize(link_path) else {
        return copy_dangling(link_path, kind, copy).map_err(couldnt);
    };
    let lead = match real.strip_prefix(source_real) {
        Ok(rest) => settled.join(rest),
        Err(_) => real.clone(),
    };
    if real.is_dir() {
        return link(copy, &lead).map_err(couldnt);
    }
    if file_link(&lead, copy).is_err() {
        fs::copy(&real, copy).map_err(|e| failed(&real, &e))?;
    }
    Ok(())
}

/// Makes again a link that leads nowhere now, as it was worded.
#[cfg(unix)]
fn copy_dangling(link_path: &Path, _kind: &fs::FileType, copy: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(link_path)?, copy)
}

/// Makes again a folder link that leads nowhere now, as a junction to where it led. A file link that leads nowhere has
/// nothing to copy if Windows won't let it be made again: it's left out, and the check before the old copy is deleted
/// names it.
#[cfg(windows)]
fn copy_dangling(link_path: &Path, kind: &fs::FileType, copy: &Path) -> io::Result<()> {
    use std::os::windows::fs::FileTypeExt;
    let points = fs::read_link(link_path)?;
    let full = match link_path.parent() {
        Some(parent) if !points.is_absolute() => parent.join(&points),
        _ => points,
    };
    if kind.is_symlink_dir() {
        junction::create(&full, copy)
    } else {
        let _ = std::os::windows::fs::symlink_file(&full, copy);
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn copy_dangling(_link_path: &Path, _kind: &fs::FileType, _copy: &Path) -> io::Result<()> {
    Ok(())
}
