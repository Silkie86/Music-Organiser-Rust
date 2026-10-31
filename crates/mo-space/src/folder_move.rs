//! Moving a plugin's sounds or presets to another drive without the plugin noticing: the C# app's `FolderMove`
//! (Solomon, 26 Oct 2026: "a way to move some plugins data folders safely to a different drive").
//!
//! The folder is copied and every file checked, then the original is renamed to "… (old copy)" and a link (a junction
//! on Windows, which needs no administrator rights) takes its place, pointing at the new copy, so the plugin and every
//! project still find their files where they were. The old copy goes only when you say, after the plugin has opened
//! fine, and only to the Recycle Bin; [`move_back`] undoes the lot. Nothing here deletes anything of yours: the only
//! thing ever removed for good is a half-made copy the move itself made, when the move stops.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use mo_formats::paths::file_name;
use mo_library::scan::is_inside;
use serde::{Deserialize, Serialize};

use crate::clean::{Bin, Done, What, on_network, send};
use crate::plugin_space::SpaceKind;
use crate::size::{self, is_link, is_online_only, size_text};
use crate::{key, real_path};

/// What the original is called while its link stands in for it.
pub const OLD_SUFFIX: &str = " (old copy)";

/// What a folder being copied back is called until it takes the link's place.
const BACK_SUFFIX: &str = " (moving back)";

/// What a folder is called for the moment it takes to see whether Windows lets it be renamed.
const CHECK_SUFFIX: &str = " (checking)";

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
    /// The drive picked hasn't room: what it has free, and what's needed.
    NoRoom { free: u64, needs: u64 },
    /// Windows guards the folder: moving it needs administrator rights.
    NeedsAdmin,
    /// Windows wouldn't rename it (something has it open): what it said.
    InUse(String),
    /// The move was stopped; nothing changed.
    Stopped,
    /// Something went wrong copying or swapping; nothing changed: what.
    Failed(String),
}

impl CantMove {
    /// What the app says.
    pub fn text(&self) -> String {
        match self {
            CantMove::Gone => "The folder isn't there any more.".to_owned(),
            CantMove::Link => "It's moved already: it points to another place.".to_owned(),
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
            CantMove::NeedsAdmin => "Windows guards this folder, so moving it needs administrator rights, which this \
                                     version can't ask for yet. Nothing was moved."
                .to_owned(),
            CantMove::InUse(why) => {
                format!(
                    "Windows wouldn't rename the folder ({why}). Close your DAW and the plugin, then try again. Nothing was moved."
                )
            }
            CantMove::Stopped => "Stopped: nothing was moved.".to_owned(),
            CantMove::Failed(why) => format!("{why} Nothing was moved."),
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
    // they stay in its own old copy, and go when that does.
    let leave_out: Vec<String> =
        already_moved.iter().filter(|m| is_inside(&m.old_copy, &from)).map(|m| key(&m.old_copy)).collect();
    // Made here, so a folder that appeared meanwhile is never copied into (or later removed).
    fs::create_dir(&to).map_err(|e| CantMove::Failed(format!("Couldn't make {} ({e}).", to.to_string_lossy())))?;
    let copied = copy_tree(Path::new(&from), &to, false, &leave_out, progress, stop);
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

/// Undoes a move: the link goes and the old copy takes its place again, with everything saved on the other drive since
/// the move copied into it first (presets the plugin wrote through the link live only there). Once the old copy is
/// deleted, the folder is copied back instead. The copy on the other drive is left for you to remove.
pub fn move_back(moved: &MovedFolder, progress: &(dyn Fn(u64) + Sync), stop: &AtomicBool) -> Result<(), CantMove> {
    let from = moved.from.as_str();
    if !is_link(Path::new(from)) {
        return Err(CantMove::Failed(format!("{from} isn't the link the move left, so it was left alone.")));
    }
    if !fs::metadata(&moved.to).is_ok_and(|m| m.is_dir()) {
        return Err(CantMove::Failed(format!("{} isn't there.", moved.to)));
    }
    let restore = if moved.has_old_copy() {
        // Only new and newer files go into the old copy: nothing in it is removed.
        copy_tree(Path::new(&moved.to), Path::new(&moved.old_copy), true, &[], progress, stop).map_err(
            |why| match why {
                CantMove::NeedsAdmin => CantMove::Failed(
                    "Windows guards the old copy, so what the plugin saved since the move couldn't be copied into \
                     it. The plugin still uses the copy on the other drive."
                        .to_owned(),
                ),
                other => other,
            },
        )?;
        moved.old_copy.clone()
    } else {
        let back = format!("{from}{BACK_SUFFIX}");
        fs::create_dir(&back).map_err(|e| CantMove::Failed(format!("Couldn't make {back} ({e}).")))?;
        if let Err(why) = copy_tree(Path::new(&moved.to), Path::new(&back), false, &[], progress, stop) {
            remove_copy(Path::new(&back));
            return Err(why);
        }
        back
    };
    unswap(from, &restore)
}

/// Whether the old copy beside the link can go: the link is there and points at the copy on the other drive, and that
/// copy is there. Not when the drive is unplugged or the copy was moved or renamed (then the old copy is the only one
/// the plugin could use), nor when an update put a real folder back where the link was.
pub fn old_copy_can_go(moved: &MovedFolder) -> bool {
    if !moved.has_old_copy() || !fs::metadata(&moved.to).is_ok_and(|m| m.is_dir()) || !is_link(Path::new(&moved.from)) {
        return false;
    }
    let Ok(points) = fs::read_link(&moved.from) else { return false };
    let points = if points.is_absolute() {
        points
    } else {
        Path::new(&moved.from).parent().map_or(points.clone(), |p| p.join(&points))
    };
    let (Some(a), Some(b)) = (real_path(&points.to_string_lossy()), real_path(&moved.to)) else { return false };
    key(&a) == key(&b)
}

/// Sends the old copy to the Recycle Bin, once [`old_copy_can_go`] says it may. [`crate::clean::undo`] puts it back.
pub fn remove_old_copy(moved: &MovedFolder, bin: &dyn Bin) -> Done {
    let mut done = Done::default();
    if !old_copy_can_go(moved) {
        done.problems.push(crate::clean::Problem {
            path: moved.old_copy.clone(),
            why: "It's the only copy the plugin could use now (the link or the copy on the other drive isn't as the \
                  move left it), so it stays."
                .to_owned(),
        });
        return done;
    }
    let real = real_path(&moved.old_copy).unwrap_or_else(|| moved.old_copy.clone());
    send(&moved.old_copy, real, moved.bytes, What::Plugin(moved.space_kind()), bin, &mut done);
    let not_found = bin.find(&mut done.removed);
    done.problems.extend(not_found);
    done
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
        CantMove::Failed(format!(
            "The folder is called \"{}\" for now: Windows wouldn't rename it back ({e}). Rename it back in Explorer.",
            file_name(&check)
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

/// The original becomes "… (old copy)" and a link to the copy takes its name.
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
    if made.is_ok() && fs::metadata(from).is_ok_and(|m| m.is_dir()) {
        return Ok(());
    }
    // The link didn't take: the original goes back as it was.
    clear_failed_link(Path::new(from));
    let back = rename_back(aside, from);
    match (made, back) {
        (_, Err(e)) => Err(CantMove::Failed(format!(
            "The link couldn't be made, and the folder is called \"{}\" for now ({e}). Rename it back in Explorer.",
            file_name(aside)
        ))),
        (Err(e), Ok(())) if denied(&e) => Err(CantMove::NeedsAdmin),
        (Err(e), Ok(())) => Err(CantMove::Failed(format!("Windows wouldn't make the link ({e})."))),
        (Ok(()), Ok(())) => Err(CantMove::Failed("The link was made but doesn't reach the copy.".to_owned())),
    }
}

/// The link goes and the folder given takes its place again. If that fails, the link goes back.
fn unswap(from: &str, restore: &str) -> Result<(), CantMove> {
    let was = fs::read_link(from).ok();
    match remove_link(Path::new(from)) {
        Ok(()) => {}
        Err(e) if denied(&e) => return Err(CantMove::NeedsAdmin),
        Err(e) => return Err(CantMove::InUse(e.to_string())),
    }
    rename_back(restore, from).map_err(|e| {
        // The link goes back, so the plugin still finds its files on the other drive.
        if let Some(was) = &was
            && fs::symlink_metadata(from).is_err()
            && link(Path::new(from), was).is_err()
        {
            clear_failed_link(Path::new(from));
        }
        if denied(&e) { CantMove::NeedsAdmin } else { CantMove::InUse(e.to_string()) }
    })
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

/// Copies a folder into another one, file by file, keeping each file's time and checking its size once written. With
/// `only_newer`, only files that are new, or newer than the one there, are copied, and nothing is removed. A link
/// inside is made again as a link, never followed. Folders in `leave_out` (keys) aren't copied. One file at a time: a
/// move usually goes to a spinning drive, where several at once is slower.
fn copy_tree(
    from: &Path,
    to: &Path,
    only_newer: bool,
    leave_out: &[String],
    progress: &(dyn Fn(u64) + Sync),
    stop: &AtomicBool,
) -> Result<(), CantMove> {
    let fail = |what: &Path, e: io::Error| {
        if denied(&e) {
            CantMove::NeedsAdmin
        } else {
            CantMove::Failed(format!("{} couldn't be copied ({e}).", what.to_string_lossy()))
        }
    };
    let mut done = 0u64;
    let mut folders = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((source_dir, copy_dir)) = folders.pop() {
        fs::create_dir_all(&copy_dir).map_err(|e| fail(&copy_dir, e))?;
        let entries = fs::read_dir(&source_dir).map_err(|e| fail(&source_dir, e))?;
        for entry in entries {
            if stop.load(Ordering::Relaxed) {
                return Err(CantMove::Stopped);
            }
            let entry = entry.map_err(|e| fail(&source_dir, e))?;
            let source = entry.path();
            if leave_out.contains(&key(&source.to_string_lossy())) {
                continue;
            }
            let copy = copy_dir.join(entry.file_name());
            let kind = entry.file_type().map_err(|e| fail(&source, e))?;
            if kind.is_symlink() {
                if !only_newer || fs::symlink_metadata(&copy).is_err() {
                    copy_link(&source, &copy)?;
                }
                continue;
            }
            if kind.is_dir() {
                folders.push((source, copy));
                continue;
            }
            let meta = entry.metadata().map_err(|e| fail(&source, e))?;
            // Never downloaded to be copied (one that went online-only after the check): it stops before the swap.
            if is_online_only(&meta) {
                return Err(CantMove::OnlineOnly);
            }
            let when = meta.modified().map_err(|e| fail(&source, e))?;
            if only_newer && fs::metadata(&copy).and_then(|m| m.modified()).is_ok_and(|there| there >= when) {
                continue;
            }
            if fs::symlink_metadata(&copy).is_ok_and(|m| m.is_dir() || m.file_type().is_symlink()) {
                return Err(CantMove::Failed(format!(
                    "{} is a folder or a link in the copy, so the file of that name couldn't go there.",
                    copy.to_string_lossy()
                )));
            }
            fs::copy(&source, &copy).map_err(|e| fail(&source, e))?;
            keep_time(&copy, when).map_err(|e| fail(&copy, e))?;
            let written = fs::metadata(&copy).map_err(|e| fail(&copy, e))?;
            if written.len() != meta.len() {
                return Err(CantMove::Failed(format!("{} didn't copy whole.", entry.file_name().to_string_lossy())));
            }
            done += meta.len();
            progress(done);
        }
    }
    Ok(())
}

/// Gives a copied file the original's time. Opened only to change its times, so a read-only file (a library's own
/// samples often are) works too.
fn keep_time(copy: &Path, when: std::time::SystemTime) -> io::Result<()> {
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

/// Makes a link in the copy pointing at the real place the original link leads to, by its whole path: a link worded
/// from where it sits ("..\Shared") would lead somewhere else from the other drive. One that leads nowhere is left
/// out; one that can't be made again (a file link needs administrator rights on Windows) is copied as what it leads to.
fn copy_link(link_path: &Path, copy: &Path) -> Result<(), CantMove> {
    let Ok(full) = fs::canonicalize(link_path) else { return Ok(()) };
    let Ok(meta) = fs::metadata(&full) else { return Ok(()) };
    if meta.is_dir() {
        return link(copy, &full).map_err(|e| {
            CantMove::Failed(format!(
                "{} is a link to {} that couldn't be made again ({e}).",
                link_path.to_string_lossy(),
                full.to_string_lossy()
            ))
        });
    }
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&full, copy);
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(&full, copy);
    #[cfg(not(any(unix, windows)))]
    let made: io::Result<()> = Err(io::Error::new(io::ErrorKind::Unsupported, "no links here"));
    if made.is_err() {
        fs::copy(&full, copy)
            .map_err(|e| CantMove::Failed(format!("{} couldn't be copied ({e}).", full.to_string_lossy())))?;
    }
    Ok(())
}
