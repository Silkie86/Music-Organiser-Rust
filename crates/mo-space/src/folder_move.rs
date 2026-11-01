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
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

use mo_formats::paths::file_name;
use mo_library::scan::is_inside;
use serde::{Deserialize, Serialize};

use crate::admin::{Admin, AsAdmin, NoAdmin, Outcome, Step, ask_for, real_spot, reason};
use crate::clean::{Bin, Done, Problem, Sending, What, on_network};
use crate::plugin_space::SpaceKind;
use crate::size::{self, is_link, is_online_only, size_text};
use crate::{key, not_there, real_folder, real_path, text};

/// What the original is called while its link stands in for it.
pub const OLD_SUFFIX: &str = " (old copy)";

/// What a folder being copied back is called until it takes the link's place.
const BACK_SUFFIX: &str = " (moving back)";

/// Where moving back keeps the old copy's files that were newer than the other drive's, beside the folder.
pub const KEPT_SUFFIX: &str = " (newer files from the old copy)";

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

/// Which drive things are on and how much room it has, and how to ask for administrator rights on one. The app uses
/// [`SystemDrives`]; tests stand in a second drive.
pub trait Drives: Sync {
    /// Whether two folders are on the same drive.
    fn same(&self, a: &Path, b: &Path) -> bool;
    /// How much room a drive has left, if it can be told.
    fn free(&self, path: &Path) -> Option<u64>;
    /// Windows' administrator question, for a folder or link it guards: the app's drives ask it; by default nothing is
    /// asked.
    fn admin(&self) -> &dyn Admin {
        &NoAdmin
    }
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
    fn admin(&self) -> &dyn Admin {
        &AsAdmin
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
                 there). Link it again instead: that brings the update's files into the copy and puts the link back. \
                 Nothing was moved."
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
                                     then try again. If Windows still says no, the folder needs administrator rights. \
                                     Nothing was moved."
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
    destination_problem(from, destination, bytes, drives, false, stop)
}

/// Whether the folder picked is the folder being moved or inside it, as written or where it really is.
fn destination_inside(from: &str, destination: &str) -> bool {
    let real_from = real_path(from).unwrap_or_else(|| from.to_owned());
    let real_destination = real_path(destination).unwrap_or_else(|| destination.to_owned());
    is_inside(destination, from) || is_inside(&real_destination, &real_from)
}

/// Why a folder (a real one, there) can't be copied into `destination`, or `None` when it can: the folder picked is
/// inside it, on a network drive or not there; the name is taken there; some of its files are only online; or there
/// isn't room for `bytes` and some to spare. On the same drive it's [`CantMove::SameDrive`], or, when `rename` (it'll be
/// renamed rather than copied), only the checks before the copying ones count.
pub(crate) fn destination_problem(
    from: &str,
    destination: &str,
    bytes: u64,
    drives: &dyn Drives,
    rename: bool,
    stop: &AtomicBool,
) -> Option<CantMove> {
    let from_path = Path::new(from);
    if destination_inside(from, destination) {
        return Some(CantMove::DestinationInside);
    }
    let real_destination = real_path(destination).unwrap_or_else(|| destination.to_owned());
    if on_network(destination) || on_network(&real_destination) {
        return Some(CantMove::NetworkDrive);
    }
    if !fs::metadata(destination).is_ok_and(|m| m.is_dir()) {
        return Some(CantMove::DestinationGone);
    }
    let same = drives.same(from_path, Path::new(destination));
    if same && !rename {
        return Some(CantMove::SameDrive);
    }
    let to = target(from, destination);
    if fs::symlink_metadata(&to).is_ok() {
        return Some(CantMove::TargetThere(to.to_string_lossy().into_owned()));
    }
    if same {
        return None;
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
    // Find out before copying whether Windows lets the folder be renamed, rather than after an hour of copying. One it
    // guards is swapped as administrator: Windows asks then.
    check_rename_or_ask(&from, drives.admin())?;
    // The old copies of folders moved from inside it before (Reaktor 6's inside Native Instruments) aren't copied:
    // they stay in its own old copy, which can't go while their links don't reach their copies.
    let leave_out: Vec<String> =
        already_moved.iter().filter(|m| is_inside(&m.old_copy, &from)).map(|m| key(&m.old_copy)).collect();
    let tree = Tree { leave_out: &leave_out, ..Tree::new(Path::new(&from), &to, &to) };
    copy_fresh(&tree, progress, stop)?;
    let aside = format!("{from}{OLD_SUFFIX}");
    if let Err(why) = swap_or_ask(&from, &to, &aside, drives.admin()) {
        // The copy goes only while nothing leads into it and the original is still there (where it was, or set
        // aside): otherwise it may be the copy the plugin uses, or the only one.
        if !is_link(Path::new(&from)) && (real_folder(&from) || real_folder(&aside)) {
            remove_copy(&to);
        }
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

/// What moving back did besides putting the folder back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MovedBack {
    /// The old copy's files that were newer than the other drive's ones (paths inside the folder): kept in `kept_in`
    /// rather than replaced, for you to look at.
    pub kept: Vec<String>,
    /// The folder they're kept in, beside the plugin's folder.
    pub kept_in: String,
    /// Links in the old copy where the other drive has a file or folder now (paths inside the folder): left as they
    /// were.
    pub links_left: Vec<String>,
}

/// Undoes a move: the link goes and the old copy takes its place again, with what changed on the other drive since the
/// move copied into it first (presets the plugin wrote through the link live only there). The old copy's own newer
/// files are kept aside, never replaced. Once the old copy is deleted, the folder is copied back instead. Either way
/// only with room to spare on this drive, and only while the link still leads to the copy the move made. Close the DAW
/// first: what it saves meanwhile is caught by a second pass, but not after the swap. Once it returns `Ok`, the move
/// is over and the app drops its record; the copy on the other drive is left for you to remove. Call it away from the
/// window's thread.
pub fn move_back(
    moved: &MovedFolder,
    drives: &dyn Drives,
    progress: &(dyn Fn(u64) + Sync),
    stop: &AtomicBool,
) -> Result<MovedBack, CantMove> {
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
    let here = settled.parent().unwrap_or(settled);
    if moved.has_old_copy() {
        // The folder a try that stopped part way kept files in is used again, so they're all in one place.
        let kept_first = format!("{from}{KEPT_SUFFIX}");
        let kept_in = PathBuf::from(if real_folder(&kept_first) { kept_first } else { fresh_name(from, KEPT_SUFFIX) });
        let tree = Tree { keep_in: Some(&kept_in), ..Tree::new(to, Path::new(&moved.old_copy), settled) };
        room_for(&tree, here, drives, stop)?;
        // From here on the old copy changes: a stop or a failure says so, and where its own newer files are.
        let partly = |why: CantMove| match why {
            CantMove::LeftHalfway(_) => why,
            other => {
                let said = if matches!(other, CantMove::Stopped) { "Stopped.".to_owned() } else { other.text() };
                let kept = if kept_in.exists() {
                    format!(", and its own newer files are kept in {}", text(&kept_in))
                } else {
                    String::new()
                };
                CantMove::LeftHalfway(format!(
                    "{} The old copy ({}) was partly brought up to date from {}{kept}. The link and {} are as they \
                     were, so it's safe to try again once the DAW is closed.",
                    said.trim_end_matches("Nothing was moved.").trim_end(),
                    moved.old_copy,
                    moved.to,
                    moved.to
                ))
            }
        };
        // New files, and files that differ, go into the old copy; nothing in it is removed.
        let first = copy_tree(&tree, Fill::Freshen, progress, stop).map_err(partly)?;
        // What the plugin saved through the link meanwhile. What the first pass wrote isn't yours: it isn't kept.
        let mut ours = first.written.clone();
        ours.sort();
        let again = copy_tree(&Tree { ours: &ours, ..tree }, Fill::Freshen, &|n| progress(first.bytes + n), stop)
            .map_err(partly)?;
        unswap_or_ask(from, &moved.old_copy, to, drives.admin()).map_err(partly)?;
        return Ok(MovedBack {
            kept: shown(&[first.kept, again.kept].concat()),
            kept_in: kept_in.to_string_lossy().into_owned(),
            links_left: shown(&[first.links_left, again.links_left].concat()),
        });
    }
    let back = format!("{from}{BACK_SUFFIX}");
    if fs::symlink_metadata(&back).is_ok() {
        return Err(CantMove::Failed(format!(
            "There's a folder called {} beside it, left from an earlier try: look in it, then delete or rename it.",
            file_name(&back)
        )));
    }
    // The whole folder comes back onto the drive the move freed.
    let tree = Tree::new(to, Path::new(&back), settled);
    room_for(&tree, here, drives, stop)?;
    let (first, again) = copy_fresh(&tree, progress, stop)?;
    if let Err(why) = unswap_or_ask(from, &back, to, drives.admin()) {
        if link_reaches(moved) {
            // As it was: the plugin still uses the other drive, so the copy made to come back goes.
            remove_copy(Path::new(&back));
        }
        return Err(why);
    }
    Ok(MovedBack { links_left: shown(&[first.links_left, again.links_left].concat()), ..MovedBack::default() })
}

/// Whether the drive `here` is on has room for what freshening `tree` copies, and some to spare. Counted only when the
/// drive's room can be told.
pub(crate) fn room_for(tree: &Tree, here: &Path, drives: &dyn Drives, stop: &AtomicBool) -> Result<(), CantMove> {
    let Some(free) = drives.free(here) else { return Ok(()) };
    let needs = copy_tree(tree, Fill::Count, &|_| {}, stop)?.bytes;
    if free < needs.saturating_add(HEADROOM) {
        return Err(CantMove::NoRoom { free, needs });
    }
    Ok(())
}

/// Copies a folder into a new folder made for it (so a folder that appeared meanwhile is never copied into, or later
/// removed), then a second time for what was saved while it copied (an hour, for a big one). If anything goes wrong,
/// the new folder goes. Returns what each pass did.
pub(crate) fn copy_fresh(
    tree: &Tree,
    progress: &(dyn Fn(u64) + Sync),
    stop: &AtomicBool,
) -> Result<(Copied, Copied), CantMove> {
    fs::create_dir(tree.to)
        .map_err(|e| CantMove::Failed(format!("Couldn't make {} ({e}).", tree.to.to_string_lossy())))?;
    let copied = copy_tree(tree, Fill::New, progress, stop).and_then(|first| {
        copy_tree(tree, Fill::Freshen, &|n| progress(first.bytes + n), stop).map(|again| (first, again))
    });
    if copied.is_err() {
        remove_copy(tree.to);
    }
    copied
}

/// `base` with `suffix` ("… (old copy)"), or, when that's taken, numbered ("… (old copy 2)").
pub(crate) fn fresh_name(base: &str, suffix: &str) -> String {
    let mut name = format!("{base}{suffix}");
    let mut n = 2;
    while fs::symlink_metadata(&name).is_ok() {
        name = format!("{base}{} {n})", suffix.trim_end_matches(')'));
        n += 1;
    }
    name
}

/// What's left of `path` after `folder`, compared part by part whichever way the slashes lean, ignoring case and
/// Windows' long-path prefix (`\\?\`), or `None` when it isn't inside.
pub(crate) fn rest_after(path: &str, folder: &str) -> Option<PathBuf> {
    let parts = |p: &str| -> Vec<String> {
        let slashes = p.replace('\\', "/");
        let plain = slashes.strip_prefix("//?/UNC/").or_else(|| slashes.strip_prefix("//?/")).unwrap_or(&slashes);
        plain.split('/').filter(|s| !s.is_empty() && *s != ".").map(str::to_owned).collect()
    };
    let (path_parts, folder_parts) = (parts(path), parts(folder));
    if folder_parts.len() > path_parts.len() || !folder_parts.iter().zip(&path_parts).all(|(a, b)| key(a) == key(b)) {
        return None;
    }
    Some(path_parts[folder_parts.len()..].iter().collect())
}

/// The first few of a list, and how many more ("a, b, c and 2 more").
pub(crate) fn listed(only: &[String]) -> String {
    let first: Vec<&str> = only.iter().take(5).map(String::as_str).collect();
    match only.len() - first.len() {
        0 => first.join(", "),
        more => format!("{} and {more} more", first.join(", ")),
    }
}

/// Paths inside the folder, once each, in order.
pub(crate) fn shown(paths: &[PathBuf]) -> Vec<String> {
    let mut shown: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    shown.sort();
    shown.dedup();
    shown
}

/// Whether the link where the folder was leads to the copy on the other drive, and that copy is there.
pub(crate) fn link_reaches(moved: &MovedFolder) -> bool {
    is_link(Path::new(&moved.from)) && leads_to(&moved.from, Path::new(&moved.to))
}

/// Whether a path ends up at a folder that's there, following any links on the way.
pub(crate) fn leads_to(path: &str, to: &Path) -> bool {
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

/// The folders moved from inside this one before it moved.
fn inner_moves<'a>(moved: &'a MovedFolder, all: &'a [MovedFolder]) -> impl Iterator<Item = &'a MovedFolder> {
    all.iter().filter(move |m| key(&m.from) != key(&moved.from) && is_inside(&m.from, &moved.from))
}

/// The old copies of folders moved from inside this one before, as they sit in `old` (where its own old copy is now),
/// with their moves. Only the ones there: an old copy inside an inner one's old copy is that one's.
fn held<'a>(old: &Path, moved: &'a MovedFolder, all: &'a [MovedFolder]) -> Vec<(PathBuf, &'a MovedFolder)> {
    inner_moves(moved, all)
        .filter_map(|m| Some((old.join(rest_after(&m.old_copy, &moved.from)?), m)))
        .filter(|(at, _)| fs::symlink_metadata(at).is_ok_and(|m| m.is_dir()))
        .collect()
}

/// Every move whose old copy sits somewhere in `old`, however deep.
fn all_held<'a>(old: &Path, moved: &'a MovedFolder, all: &'a [MovedFolder], found: &mut Vec<&'a MovedFolder>) {
    for (at, inner) in held(old, moved, all) {
        found.push(inner);
        all_held(&at, inner, all, found);
    }
}

/// What's in the old copy but not in the copy on the other drive, or newer in the old copy, as paths inside it: files
/// saved there while the move ran, or deleted from the other drive since. The old copies of folders moved from inside
/// it before are checked against their own copies. Reads both folders: call it away from the window's thread.
pub fn only_in_old_copy(moved: &MovedFolder, all: &[MovedFolder], stop: &AtomicBool) -> Result<Vec<String>, CantMove> {
    let mut only = Vec::new();
    let old = Path::new(&moved.old_copy);
    differences((old, Path::new(&moved.to)), held(old, moved, all), all, Path::new(""), &mut only, stop)?;
    only.sort();
    Ok(only)
}

/// What's in `old` but not in `copy`, or newer in `old`, as paths inside it.
pub(crate) fn only_in(old: &Path, copy: &Path, stop: &AtomicBool) -> Result<Vec<String>, CantMove> {
    let mut only = Vec::new();
    differences((old, copy), Vec::new(), &[], Path::new(""), &mut only, stop)?;
    only.sort();
    Ok(only)
}

/// Adds what's in `old` but not in `copy`, or newer in `old`, as paths under `shown_as`. `inner` holds the old copies
/// of folders moved from inside it before, as they sit in `old`: each is checked against its own copy.
fn differences(
    (old, copy): (&Path, &Path),
    inner: Vec<(PathBuf, &MovedFolder)>,
    all: &[MovedFolder],
    shown_as: &Path,
    only: &mut Vec<String>,
    stop: &AtomicBool,
) -> Result<(), CantMove> {
    let skip: Vec<String> = inner.iter().map(|(at, _)| key(&at.to_string_lossy())).collect();
    let show = |p: &Path| shown_as.join(p.strip_prefix(old).unwrap_or(p)).to_string_lossy().into_owned();
    let mut folders = vec![(old.to_path_buf(), copy.to_path_buf())];
    while let Some((old_dir, copy_dir)) = folders.pop() {
        let Ok(entries) = fs::read_dir(&old_dir) else {
            only.push(show(&old_dir));
            continue;
        };
        for entry in entries {
            if stop.load(Ordering::Relaxed) {
                return Err(CantMove::Stopped);
            }
            let Ok(entry) = entry else {
                only.push(show(&old_dir));
                continue;
            };
            let path = entry.path();
            if skip.contains(&key(&path.to_string_lossy())) {
                continue;
            }
            let there = copy_dir.join(entry.file_name());
            let Ok(kind) = entry.file_type() else {
                only.push(show(&path));
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
                only.push(show(&path));
            }
        }
    }
    for (at, m) in inner {
        let rest = at.strip_prefix(old).unwrap_or(&at).to_path_buf();
        differences((&at, Path::new(&m.to)), held(&at, m, all), all, &shown_as.join(rest), only, stop)?;
    }
    Ok(())
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
    let mut inner = Vec::new();
    all_held(Path::new(&moved.old_copy), moved, all, &mut inner);
    if let Some(lost) = inner.into_iter().find(|m| !link_reaches(m)) {
        return stays(
            moved,
            &format!(
                "It holds the old copy of {}, whose link doesn't lead to {} now, so it stays.",
                file_name(&lost.from),
                lost.to
            ),
        );
    }
    if !sure {
        match only_in_old_copy(moved, all, stop) {
            Err(_) => return stays(moved, "Stopped before it was all checked, so it stays."),
            Ok(only) if !only.is_empty() => {
                return stays(
                    moved,
                    &format!(
                        "Some of it is only in the old copy, or newer there: {}. It may have been saved while the move \
                         ran, or deleted from the other drive since. Look before deleting it.",
                        listed(&only)
                    ),
                );
            }
            Ok(_) => {}
        }
    }
    let mut done = Done::default();
    // The folder itself (it was checked not to be a link), not where a link would lead.
    let real = real_spot(&moved.old_copy);
    // Checked just now; one Windows guards is checked again by the administrator step, against what it held now.
    let mut sending = Sending::new(bin, stop);
    sending.send(&moved.old_copy, real, moved.bytes, What::Plugin(moved.space_kind()), &mut done, || Ok(()));
    sending.finish(&mut done);
    done
}

/// The old copy stays, and why.
fn stays(moved: &MovedFolder, why: &str) -> Done {
    Done { problems: vec![Problem { path: moved.old_copy.clone(), why: why.to_owned() }], ..Done::default() }
}

/// Whether Windows said no because of rights (access denied), rather than something having the folder open.
pub(crate) fn denied(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied
}

/// [`check_rename`], except that a folder Windows guards is fine when `admin` can ask: the swap is done as
/// administrator then.
pub(crate) fn check_rename_or_ask(from: &str, admin: &dyn Admin) -> Result<(), CantMove> {
    match check_rename(from) {
        Err(CantMove::NeedsAdmin) if admin.can_ask() => Ok(()),
        other => other,
    }
}

/// Renames the folder and straight back, to learn whether the swap will work.
pub(crate) fn check_rename(from: &str) -> Result<(), CantMove> {
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
pub(crate) fn swap(from: &str, to: &Path, aside: &str) -> Result<(), CantMove> {
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

/// [`swap`], as administrator when Windows guards the folder (Windows asks you then). What's there afterwards decides
/// how it went: swapped, as it was, or neither, in which case it says what to look at.
pub(crate) fn swap_or_ask(from: &str, to: &Path, aside: &str, admin: &dyn Admin) -> Result<(), CantMove> {
    match swap(from, to, aside) {
        Err(CantMove::NeedsAdmin) if admin.can_ask() => {}
        other => return other,
    }
    let said = ask_for(admin, Step::Swap, &[vec![real_spot(from), text(to), real_spot(aside)]]);
    if is_link(Path::new(from)) && leads_to(from, to) && real_folder(aside) {
        return Ok(());
    }
    if real_folder(from) && not_there(Path::new(aside)) {
        return Err(as_it_was(said, "The administrator step finished, but the folder wasn't swapped."));
    }
    Err(not_as_expected(
        said,
        format!(
            "After the administrator step, {from} should be a link to {} and the original folder should be called \
             \"{}\", but it isn't quite: look at both before trying again.",
            text(to),
            file_name(aside)
        ),
    ))
}

/// [`unswap`], as administrator when Windows guards the link or the folder (Windows asks you then). What's there
/// afterwards decides how it went.
pub(crate) fn unswap_or_ask(from: &str, restore: &str, to: &Path, admin: &dyn Admin) -> Result<(), CantMove> {
    match unswap(from, restore, to) {
        Err(CantMove::NeedsAdmin) if admin.can_ask() => {}
        other => return other,
    }
    let said = ask_for(admin, Step::Unswap, &[vec![real_spot(from), real_spot(restore), text(to)]]);
    if real_folder(from) && not_there(Path::new(restore)) {
        return Ok(());
    }
    if is_link(Path::new(from)) && leads_to(from, to) && real_folder(restore) {
        return Err(as_it_was(said, "The administrator step finished, but the folder wasn't put back."));
    }
    Err(not_as_expected(
        said,
        format!(
            "After the administrator step, \"{}\" should be called \"{}\" again, in place of the link, but it isn't \
             quite: look at both before trying again. The plugin's files are in {} too.",
            file_name(restore),
            file_name(from),
            text(to)
        ),
    ))
}

/// Nothing changed: why, as the administrator step (or the question) said it.
fn as_it_was(said: Result<Vec<Outcome>, String>, otherwise: &str) -> CantMove {
    CantMove::Failed(reason(&said, 0, otherwise))
}

/// Things changed, but not as they should have: what the step said about it, or what to look at. Unlike [`reason`],
/// only a step's "halfway" says what's where; why it kept something, or why it didn't run, wouldn't explain a change.
fn not_as_expected(said: Result<Vec<Outcome>, String>, otherwise: String) -> CantMove {
    match said.map(|s| s.into_iter().next()) {
        Ok(Some(Outcome::Halfway(what))) => CantMove::LeftHalfway(what),
        _ => CantMove::LeftHalfway(otherwise),
    }
}

/// The link goes and `restore` takes its name. If that fails, the link to `to` goes back; if even that fails, says
/// what to rename.
pub(crate) fn unswap(from: &str, restore: &str, to: &Path) -> Result<(), CantMove> {
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
pub(crate) fn link(at: &Path, target: &Path) -> io::Result<()> {
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
pub(crate) fn clear_failed_link(at: &Path) {
    if is_link(at) {
        let _ = remove_link(at);
    } else if fs::symlink_metadata(at).is_ok_and(|m| m.is_dir()) {
        let _ = fs::remove_dir(at);
    }
}

/// Removes a link only, never what it leads to.
pub(crate) fn remove_link(at: &Path) -> io::Result<()> {
    if !is_link(at) {
        return Err(io::Error::other("not a link"));
    }
    // A junction or a folder link is removed as a folder on Windows; a symbolic link as a file elsewhere.
    if cfg!(windows) { fs::remove_dir(at) } else { fs::remove_file(at) }
}

/// Removes a copy the move made itself, when the move stops: never a link, never the original.
pub(crate) fn remove_copy(path: &Path) {
    if fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) && !is_link(path) {
        let _ = fs::remove_dir_all(path);
    }
}

/// How [`copy_tree`] treats what's in the copy already.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fill {
    /// The copy was just made, so anything there already is a second file of the same name but for capitals, on a
    /// drive that doesn't tell them apart: the copy stops.
    New,
    /// The copy has most of it: files that are new, or differ in size or time, are copied, each under a temporary name
    /// first; nothing is removed, and links there stay as they are.
    Freshen,
    /// Nothing is copied: only the bytes [`Fill::Freshen`] would copy are added up.
    Count,
}

/// A folder to copy, and where.
pub(crate) struct Tree<'a> {
    pub(crate) from: &'a Path,
    pub(crate) to: &'a Path,
    /// Where `to` ends up once everything's done: links leading inside `from` lead to the same place inside it.
    pub(crate) settled: &'a Path,
    /// Where files in `to` that are newer than the ones replacing them go first (by their paths inside `to`), when
    /// `to` is yours; without it they're replaced (a copy the move made).
    pub(crate) keep_in: Option<&'a Path>,
    /// With `keep_in`: every file in `to` that's replaced is kept there first, not only newer ones.
    pub(crate) keep_all: bool,
    /// With `keep_in`: files an earlier pass wrote into `to` (paths inside it, sorted). They're the copy's own work,
    /// not yours, so they're replaced without being kept.
    pub(crate) ours: &'a [PathBuf],
    /// Folders not copied (keys).
    pub(crate) leave_out: &'a [String],
}

impl<'a> Tree<'a> {
    /// `from` copied into `to`, which ends up as `settled`; nothing kept aside or left out.
    pub(crate) fn new(from: &'a Path, to: &'a Path, settled: &'a Path) -> Self {
        Tree { from, to, settled, keep_in: None, keep_all: false, ours: &[], leave_out: &[] }
    }
}

/// What a copy did: bytes copied (or that would be, counting), and what it kept aside or left (paths inside the copy).
#[derive(Default)]
pub(crate) struct Copied {
    pub(crate) bytes: u64,
    pub(crate) kept: Vec<PathBuf>,
    pub(crate) links_left: Vec<PathBuf>,
    /// With `keep_in`, freshening: the files it wrote (paths inside the copy), for [`Tree::ours`] in the next pass.
    pub(crate) written: Vec<PathBuf>,
}

/// Copies a folder into another one, file by file, keeping each file's time and checking its size once written. A link
/// inside is made again as a link, never followed. A file or folder that goes while it's read (a plugin's lock file)
/// is passed over. One file at a time: a move usually goes to a spinning drive, where several at once is slower.
pub(crate) fn copy_tree(
    tree: &Tree,
    fill: Fill,
    progress: &(dyn Fn(u64) + Sync),
    stop: &AtomicBool,
) -> Result<Copied, CantMove> {
    let source_real = fs::canonicalize(tree.from).map_err(|e| failed(tree.from, &e))?;
    let inside = |copy: &Path| copy.strip_prefix(tree.to).unwrap_or(copy).to_path_buf();
    let mut copied = Copied::default();
    let mut folders = vec![(tree.from.to_path_buf(), tree.to.to_path_buf())];
    while let Some((source_dir, copy_dir)) = folders.pop() {
        let entries = match fs::read_dir(&source_dir) {
            Ok(entries) => entries,
            Err(_) if gone(&source_dir) => continue,
            Err(e) => return Err(failed(&source_dir, &e)),
        };
        for entry in entries {
            if stop.load(Ordering::Relaxed) {
                return Err(CantMove::Stopped);
            }
            let entry = entry.map_err(|e| failed(&source_dir, &e))?;
            let source = entry.path();
            if tree.leave_out.contains(&key(&source.to_string_lossy())) {
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
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) if gone(&source) => continue,
                Err(e) => return Err(failed(&source, &e)),
            };
            // A link there stays as it is: nothing is written through it (it could lead into another plugin's folder).
            if there.as_ref().is_some_and(|m| m.file_type().is_symlink()) {
                if !kind.is_symlink() {
                    copied.links_left.push(inside(&copy));
                }
                continue;
            }
            if kind.is_symlink() {
                if there.is_none() && fill != Fill::Count {
                    match copy_link(&source, &kind, &copy, tree, &source_real) {
                        Err(_) if gone(&source) => {}
                        other => other?,
                    }
                }
                continue;
            }
            if kind.is_dir() {
                match &there {
                    None if fill == Fill::Count => {}
                    None => fs::create_dir(&copy).map_err(|e| failed(&copy, &e))?,
                    Some(m) if m.is_dir() => {}
                    Some(_) => {
                        return Err(CantMove::Failed(format!(
                            "{} is a file in the copy, so the folder of that name couldn't go there.",
                            copy.to_string_lossy()
                        )));
                    }
                }
                folders.push((source, copy));
                continue;
            }
            let meta = match entry.metadata() {
                Ok(meta) => meta,
                Err(_) if gone(&source) => continue,
                Err(e) => return Err(failed(&source, &e)),
            };
            // Never downloaded to be copied (one that went online-only after the check): it stops before the swap.
            if is_online_only(&meta) {
                return Err(CantMove::OnlineOnly);
            }
            if let Some(m) = &there {
                if m.is_dir() {
                    return Err(CantMove::Failed(format!(
                        "{} is a folder in the copy, so the file of that name couldn't go there.",
                        copy.to_string_lossy()
                    )));
                }
                if same_file(m, &meta) {
                    continue;
                }
            }
            if fill == Fill::Count {
                copied.bytes += meta.len();
                continue;
            }
            if let (Some(m), Some(keep_in)) = (&there, tree.keep_in) {
                let rest = inside(&copy);
                // Yours, and newer than the one coming in (or any, with `keep_all`): kept aside, never replaced.
                if (tree.keep_all || newer(m, &meta)) && tree.ours.binary_search(&rest).is_err() {
                    keep_aside(&copy, &keep_in.join(&rest))?;
                    copied.kept.push(rest);
                }
            }
            match copy_file(&source, &copy, &meta, fill == Fill::Freshen) {
                Err(_) if gone(&source) => {
                    if fill == Fill::New {
                        let _ = fs::remove_file(&copy);
                    }
                    continue;
                }
                other => other?,
            }
            if tree.keep_in.is_some() && fill == Fill::Freshen {
                copied.written.push(inside(&copy));
            }
            copied.bytes += meta.len();
            progress(copied.bytes);
        }
    }
    Ok(copied)
}

/// Whether a path is no longer there.
fn gone(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err()
}

/// What a copy that went wrong says.
pub(crate) fn failed(path: &Path, e: &io::Error) -> CantMove {
    CantMove::Failed(format!("Copying stopped at {} ({e}).", path.to_string_lossy()))
}

/// Moves a file of yours out of the way, to the same place inside the folder kept beside the plugin's.
fn keep_aside(file: &Path, keep: &Path) -> Result<(), CantMove> {
    if fs::symlink_metadata(keep).is_ok() {
        return Err(CantMove::Failed(format!("{} is there already.", keep.to_string_lossy())));
    }
    if let Some(parent) = keep.parent() {
        fs::create_dir_all(parent).map_err(|e| failed(parent, &e))?;
    }
    fs::rename(file, keep).map_err(|e| failed(file, &e))
}

/// Copies one file, keeps its time and checks it arrived whole. `through_temp` (into a folder of yours): it's written
/// under a temporary name first and takes its place only once whole, so one that fails leaves what was there as it
/// was.
fn copy_file(source: &Path, copy: &Path, meta: &fs::Metadata, through_temp: bool) -> Result<(), CantMove> {
    let when = meta.modified().map_err(|e| failed(source, &e))?;
    if !through_temp {
        return fs::copy(source, copy)
            .and_then(|_| keep_time(copy, when))
            .map_err(|e| failed(source, &e))
            .and_then(|()| whole(copy, meta, source));
    }
    let mut name = copy.file_name().unwrap_or_default().to_os_string();
    name.push(PART_SUFFIX);
    let part = copy.with_file_name(name);
    match fs::symlink_metadata(&part) {
        Ok(m) if !m.is_file() => {
            return Err(CantMove::Failed(format!("{} is in the way.", part.to_string_lossy())));
        }
        // Left by a copy that stopped.
        Ok(_) => remove_part(&part),
        Err(_) => {}
    }
    let written = fs::copy(source, &part)
        .and_then(|_| keep_time(&part, when))
        .map_err(|e| failed(source, &e))
        .and_then(|()| whole(&part, meta, source))
        .and_then(|()| put_in_place(&part, copy));
    if written.is_err() {
        remove_part(&part);
    }
    written
}

/// Whether a copy is as big as the file it came from.
fn whole(written: &Path, meta: &fs::Metadata, source: &Path) -> Result<(), CantMove> {
    match fs::metadata(written) {
        Ok(m) if m.len() == meta.len() => Ok(()),
        Ok(_) => Err(CantMove::Failed(format!("{} didn't copy whole.", source.to_string_lossy()))),
        Err(e) => Err(failed(written, &e)),
    }
}

/// Puts a whole copy in place of the file there, a read-only one too (Windows won't replace those otherwise).
fn put_in_place(part: &Path, copy: &Path) -> Result<(), CantMove> {
    let was_read_only = set_read_only(copy, false);
    fs::rename(part, copy).map_err(|e| {
        if was_read_only {
            set_read_only(copy, true);
        }
        failed(copy, &e)
    })
}

/// Removes a temporary copy, a read-only one too.
fn remove_part(part: &Path) {
    set_read_only(part, false);
    let _ = fs::remove_file(part);
}

/// Windows only (elsewhere a read-only file can be replaced or removed anyway): makes a file read-only or not, and
/// says whether it changed.
fn set_read_only(path: &Path, read_only: bool) -> bool {
    #[cfg(windows)]
    {
        let Ok(meta) = fs::symlink_metadata(path) else { return false };
        if !meta.is_file() || meta.permissions().readonly() == read_only {
            return false;
        }
        let mut permissions = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(read_only);
        fs::set_permissions(path, permissions).is_ok()
    }
    #[cfg(not(windows))]
    {
        let _ = (path, read_only);
        false
    }
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

/// Where a link leads in the copy: the same place, by its whole path (a link worded from where it sits, "..\Shared",
/// would lead somewhere else from the other drive), or, for a place inside the folder being copied, the same place
/// inside the folder the copy becomes.
fn lead(place: PathBuf, inside: &Path, tree: &Tree) -> PathBuf {
    match place.strip_prefix(inside) {
        Ok(rest) => tree.settled.join(rest),
        Err(_) => place,
    }
}

/// Makes a link in the copy leading where the original leads ([`lead`]). One that leads nowhere now (a drive that
/// isn't plugged in) is made again too. A file link Windows won't let be made again is copied as the file it leads to.
fn copy_link(
    link_path: &Path,
    kind: &fs::FileType,
    copy: &Path,
    tree: &Tree,
    source_real: &Path,
) -> Result<(), CantMove> {
    let couldnt = |e: io::Error| {
        CantMove::Failed(format!("{} is a link that couldn't be made again ({e}).", link_path.to_string_lossy()))
    };
    let Ok(real) = fs::canonicalize(link_path) else {
        // Where it would lead, worked out from its wording alone.
        let points = fs::read_link(link_path).map_err(couldnt)?;
        let place = tidy(&link_path.parent().map_or(points.clone(), |p| p.join(&points)));
        return copy_dangling(kind, &lead(place, &tidy(tree.from), tree), copy).map_err(couldnt);
    };
    let to = lead(real.clone(), source_real, tree);
    if real.is_dir() {
        return link(copy, &to).map_err(couldnt);
    }
    if file_link(&to, copy).is_err() {
        fs::copy(&real, copy).map_err(|e| failed(&real, &e))?;
    }
    Ok(())
}

/// A path with its "." and ".." worked out from the words alone.
fn tidy(path: &Path) -> PathBuf {
    let mut tidy = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                tidy.pop();
            }
            other => tidy.push(other.as_os_str()),
        }
    }
    tidy
}

/// Makes again a link that leads nowhere now.
#[cfg(not(windows))]
fn copy_dangling(_kind: &fs::FileType, to: &Path, copy: &Path) -> io::Result<()> {
    file_link(to, copy)
}

/// Makes again a link that leads nowhere now: a folder link as a junction. A file link has nothing to copy if Windows
/// won't let it be made again: it's left out, and the check before the old copy is deleted names it.
#[cfg(windows)]
fn copy_dangling(kind: &fs::FileType, to: &Path, copy: &Path) -> io::Result<()> {
    use std::os::windows::fs::FileTypeExt;
    if kind.is_symlink_dir() {
        junction::create(to, copy)
    } else {
        let _ = std::os::windows::fs::symlink_file(to, copy);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::rest_after;

    #[test]
    fn the_rest_of_a_path_is_found_part_by_part() {
        let rest = |path: &str, folder: &str| rest_after(path, folder).map(|p| p.to_string_lossy().replace('\\', "/"));
        assert_eq!(rest(r"D:\Plugin Data\NI\Reaktor 6", r"d:/plugin data"), Some("NI/Reaktor 6".to_owned()));
        // Windows' long-path way of writing it.
        assert_eq!(rest(r"\\?\D:\Plugin Data\NI", r"D:\Plugin Data\"), Some("NI".to_owned()));
        assert_eq!(rest(r"\\server\share\Data\NI", r"\\?\UNC\server\share\Data"), Some("NI".to_owned()));
        assert_eq!(rest_after(r"D:\Plugin Data", r"D:\Plugin Data"), Some(PathBuf::new()));
        // A name that only starts the same isn't inside.
        assert_eq!(rest_after(r"D:\Plugin Data 2\NI", r"D:\Plugin Data"), None);
        assert_eq!(rest_after(r"D:\Plugin", r"D:\Plugin Data"), None);
    }
}
