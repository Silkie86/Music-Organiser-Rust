//! Moving a plugin's sounds or presets to another drive without the plugin noticing: the C# app's `FolderMove`
//! (Solomon, 26 Oct 2026: "a way to move some plugins data folders safely to a different drive").
//!
//! The folder is copied and every file checked, then the original is renamed to "… (old copy)" and a link (a junction
//! on Windows, which needs no administrator rights) takes its place, pointing at the new copy, so the plugin and every
//! project still find their files where they were. The old copy goes only when you say, after the plugin has opened
//! fine, only to the Recycle Bin, and only once nothing is only in it; [`move_back`] undoes the lot. Nothing here
//! deletes anything of yours: the only things ever removed for good are copies the move itself made, when it stops, or
//! when a later try finds one that a crash cut off ([`crate::half_copy`]: each new copy holds a marker until it's in
//! use).

use std::collections::{HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use mo_formats::paths::file_name;
use mo_library::scan::is_inside;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::admin::{Admin, AsAdmin, NoAdmin, Outcome, Step, ask_for, real_spot, reason};
use crate::clean::{Bin, Done, on_network};
use crate::half_copy::{
    self, Claim, MARKER, busy_now, claim, clear_own, going_into_use, not_in_use, own_room, remove_marked, unmark,
};
use crate::plugin_space::SpaceKind;
use crate::size::{self, is_link, is_online_only, size_text};
use crate::{is_dir, key, not_there, real_folder, real_path, really, text};

/// What the original is called while its link stands in for it.
pub const OLD_SUFFIX: &str = " (old copy)";

/// What a folder being copied back is called until it takes the link's place.
const BACK_SUFFIX: &str = " (moving back)";

/// Where moving back keeps the old copy's files that were newer than the other drive's, beside the folder.
pub const KEPT_SUFFIX: &str = " (newer files from the old copy)";

/// What a folder is called for the moment it takes to see whether Windows lets it be renamed.
const CHECK_SUFFIX: &str = " (checking)";

/// What a file going in over an older one is called until it's whole.
pub(crate) const PART_SUFFIX: &str = ".mo-copying";

/// How far apart two times of the same file can be when one is kept to the second: FAT32 keeps times to two seconds
/// (and some drives to one).
const SAME_TIME: Duration = Duration::from_secs(2);
/// When one is kept to a hundredth of a second (exFAT).
const SAME_TIME_EXFAT: Duration = Duration::from_millis(10);
/// When both are kept finer (NTFS keeps a ten-millionth of a second; a Mac and Linux, a billionth).
const SAME_TIME_FINE: Duration = Duration::from_micros(1);

/// How long before a copy goes into use the small files it wrote are written through to the disk
/// ([`write_through`]): what was written earlier is taken to be on the disk by then (Windows writes what's waiting
/// within seconds, and less than a minute or so behind even to a slow drive while a copy keeps it busy). Big files are
/// all written through, however long ago.
const WRITTEN_THROUGH: Duration = Duration::from_secs(120);

/// The most small files written through to the disk just before a copy goes into use: the last ones written. A copy
/// whose last two minutes wrote more (tens of thousands of presets to a fast drive) has the earlier ones taken to be on
/// the disk already, as everything written before then is.
const THROUGH_MOST: usize = 20_000;

/// Files copied into a folder of yours wait under their temporary names until this many are waiting, or
/// [`WAITING_BYTES`] of them, and are then written through to the disk together and put in place ([`Put::Through`]).
const WAITING_FILES: usize = 256;

/// See [`WAITING_FILES`].
const WAITING_BYTES: u64 = 1 << 30;

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
    /// The size of the pieces a drive keeps files in (each file takes a whole number of them: 128 KB on a big exFAT
    /// drive), or 0 when it can't be told.
    fn piece(&self, _path: &Path) -> u64 {
        0
    }
    /// Whether a drive holds links (junctions, symbolic links): an exFAT or FAT32 one (a USB stick, a memory card,
    /// many an external drive) doesn't, so a link inside a folder can't be made again there, nor can the link a moved
    /// folder leaves in its place. Yes when it can't be told.
    fn holds_links(&self, _path: &Path) -> bool {
        true
    }
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
        // Where it really is: Windows finds the drive from the path's text, so a link on the way would give the room
        // on the drive the link sits on.
        fs4::available_space(really(path)).ok()
    }
    fn piece(&self, path: &Path) -> u64 {
        fs4::allocation_granularity(really(path)).unwrap_or(0)
    }
    fn holds_links(&self, path: &Path) -> bool {
        drive_holds_links(&really(path))
    }
    fn admin(&self) -> &dyn Admin {
        &AsAdmin
    }
}

/// Whether the drive a folder (where it really is) is on holds links, by its kind in Windows' list of drives
/// ([`kind_holds_links`]). One that isn't in the list (a network drive, or a CD or DVD: the list has only the drives
/// in the computer and USB sticks or memory cards) can't be told, and counts as holding them.
#[cfg(windows)]
fn drive_holds_links(real: &Path) -> bool {
    crate::clean::volume_of(&text(real)).is_none_or(|v| kind_holds_links(&v.kind))
}

/// Whether a kind of drive, as Windows names it, holds links: NTFS and ReFS do, and so does Btrfs (through another's
/// driver, which keeps them). Any other kind doesn't count as holding them: FAT, FAT32 and exFAT have nowhere to keep
/// them, and one the app doesn't know (a Mac or Linux drive read through another's driver) may not, so a move that
/// needs a link there is refused before anything is copied, not part way. One with no name can't be told, and counts
/// as holding them.
#[cfg(windows)]
fn kind_holds_links(kind: &str) -> bool {
    kind.is_empty() || ["NTFS", "ReFS", "Btrfs"].iter().any(|holds| kind.eq_ignore_ascii_case(holds))
}

/// Linux says what kind of drive a folder is on by a number: an exFAT or FAT one (FAT32 is FAT's) holds no links. Any
/// other, or one that can't be told, counts as holding them.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn drive_holds_links(real: &Path) -> bool {
    use rustix::fs::FsWord;
    const EXFAT: u32 = 0x2011_BAB0;
    const FAT: u32 = 0x4D44;
    !rustix::fs::statfs(real).is_ok_and(|s| s.f_type == EXFAT as FsWord || s.f_type == FAT as FsWord)
}

/// Elsewhere it isn't asked: a Mac keeps links even on a FAT drive (as small files), and what its exFAT drives do
/// isn't known here, so every drive counts as holding them.
#[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
fn drive_holds_links(_real: &Path) -> bool {
    true
}

/// Unix: the same device. Windows: the same drive (by its id), where the folders really are; or, when Windows can't
/// say, the same drive letter or network share.
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
    let real = |p: &Path| real_path(&p.to_string_lossy()).unwrap_or_else(|| p.to_string_lossy().into_owned());
    let (a, b) = (real(a), real(b));
    // By the drive's own id: a drive mounted in a folder of C: ("C:\Samples") is another drive, though its paths start
    // with C:. Only when Windows can't say for both is the letter (or the network share) compared.
    #[cfg(windows)]
    {
        use crate::clean::volume_of_path;
        if let (Some((_, va)), Some((_, vb))) = (volume_of_path(Path::new(&a)), volume_of_path(Path::new(&b))) {
            return va.eq_ignore_ascii_case(&vb);
        }
    }
    let root = |real: &str| {
        let k = key(real);
        if let Some(share) = k.strip_prefix("//") {
            // //server/share
            let mut parts = share.splitn(3, '/');
            format!("//{}/{}", parts.next().unwrap_or_default(), parts.next().unwrap_or_default())
        } else {
            k.split('/').next().unwrap_or_default().to_owned()
        }
    };
    root(a.as_str()) == root(b.as_str())
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
    /// It's one of your project or sample folders, holds one, or is inside one: those never move (rule 2).
    Protected,
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
    /// A file link in it leads to a file that's only online in OneDrive, and the link can't be made again where it
    /// goes, so the file would be copied: the link, and the file.
    LinkOnlineOnly { link: String, file: String },
    /// A link in it would be made again as a link where it goes, on a drive that holds none (exFAT, FAT32): the link.
    NoLinksThere(String),
    /// It's on a drive that holds no links, so the link that would take its place couldn't be made.
    NoLinksHere,
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
    /// There's a half copy where the copy goes, left by an earlier try that was cut off (the app or the computer
    /// stopped), and it stays ([`crate::half_copy`]): where, and why.
    HalfCopy { path: String, why: String },
}

impl CantMove {
    /// What the app says.
    pub fn text(&self) -> String {
        match self {
            CantMove::Gone => "The folder isn't there any more.".to_owned(),
            CantMove::Protected => "It's one of your project or sample folders, holds one, or is inside one, and \
                                    those are never moved. Nothing was moved."
                .to_owned(),
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
            CantMove::LinkOnlineOnly { link, file } => format!(
                "{link} is a link to {file}, which is only online in OneDrive. The link can't be made again where it's \
                 going (that takes Developer Mode, and a drive that holds links: an exFAT or FAT32 one doesn't), so \
                 the file would be copied, and copying it would download it first. In File Explorer, right-click \
                 {file} and pick Always keep on this device. Once it's downloaded, try again. Nothing was moved."
            ),
            CantMove::NoLinksThere(link) => format!(
                "{link} is a link, and the drive it would be copied to can't hold links (an exFAT or FAT32 drive \
                 can't), so it couldn't be made again there. Nothing was moved."
            ),
            CantMove::NoLinksHere => "The folder is on a drive that can't hold links (an exFAT or FAT32 drive can't), \
                                      so the link that takes its place when it moves couldn't be made there. Nothing \
                                      was moved."
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
            CantMove::HalfCopy { path, why } => format!(
                "There's a half copy at {path}, left when an earlier try was cut off part way, and it stays: {why} \
                 Nothing was moved."
            ),
        }
    }
}

/// Why the folder can't move to `destination`, or `None` when it can. `bytes` is its size; `moved` is the folders moved
/// already; `protected`, your project and sample folders (rule 2: nothing in them moves). Reads the folder to look for
/// OneDrive files that are only online: call it away from the window's thread.
pub fn problem(
    from: &str,
    destination: &str,
    bytes: u64,
    drives: &dyn Drives,
    moved: &[MovedFolder],
    protected: &[String],
    stop: &AtomicBool,
) -> Option<CantMove> {
    problem_with(from, destination, bytes, drives, (moved, protected), None, stop)
}

/// Whether a folder is one of your project or sample folders, holds one, or is inside one, as written or where it
/// really is: then it never moves (its old copy would be offered for the Recycle Bin afterwards).
fn protected_folder(from: &str, protected: &[String]) -> bool {
    let real_from = real_path(from).unwrap_or_else(|| from.to_owned());
    protected.iter().filter(|p| !p.trim().is_empty()).any(|p| {
        let real_p = real_path(p).unwrap_or_else(|| p.clone());
        is_inside(from, p) || is_inside(p, from) || is_inside(&real_from, &real_p) || is_inside(&real_p, &real_from)
    })
}

/// [`problem`], for a move that holds `mine`, its claim on the copy it makes: its own claim doesn't count as the app
/// being busy there.
fn problem_with(
    from: &str,
    destination: &str,
    bytes: u64,
    drives: &dyn Drives,
    (moved, protected): (&[MovedFolder], &[String]),
    mine: Option<&Claim>,
    stop: &AtomicBool,
) -> Option<CantMove> {
    let from = from.trim_end_matches(['/', '\\']);
    let from_path = Path::new(from);
    if is_link(from_path) {
        return Some(CantMove::Link);
    }
    if protected_folder(from, protected) {
        return Some(CantMove::Protected);
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
    // The link that takes its place is made where it is: found now, not once it's all copied.
    if !drives.holds_links(from_path.parent().unwrap_or(from_path)) {
        return Some(CantMove::NoLinksHere);
    }
    destination_problem(from, destination, (bytes, &left_out(from, moved)), moved, drives, false, mine, stop)
}

/// The old copies of folders moved from inside `from` before (Reaktor 6's inside Native Instruments), as keys: they
/// aren't copied when it moves, but stay in its own old copy, which can't go while their links don't reach their copies.
fn left_out(from: &str, moved: &[MovedFolder]) -> Vec<String> {
    moved.iter().filter(|m| is_inside(&m.old_copy, from)).map(|m| key(&m.old_copy)).collect()
}

/// Whether the folder picked is the folder being moved or inside it, as written or where it really is.
fn destination_inside(from: &str, destination: &str) -> bool {
    let real_from = real_path(from).unwrap_or_else(|| from.to_owned());
    let real_destination = real_path(destination).unwrap_or_else(|| destination.to_owned());
    is_inside(destination, from) || is_inside(&real_destination, &real_from)
}

/// Why a folder (a real one, there) can't be copied into `destination`, or `None` when it can: the folder picked is
/// inside it, on a network drive or not there; the name is taken there (by anything but a half copy an earlier try at
/// this copy left, cut off part way, which the copy clears first: its room counts as free); some of its files are only
/// online (or a file a link in it leads to is, and would be copied); a link in it can't be made again there (the drive
/// holds none); or there isn't room for what's copied (counted, each file and folder as the drive keeps it, leaving out
/// `leave_out`; `bytes`, its measured size, when it can't be counted) and some to spare. On the same drive it's
/// [`CantMove::SameDrive`], or, when `rename` (it'll be renamed rather than copied), only the checks before the copying
/// ones count. `moved` is every moved folder; `mine`, the claim the move holds on the copy's place, if any.
#[allow(clippy::too_many_arguments)]
pub(crate) fn destination_problem(
    from: &str,
    destination: &str,
    (bytes, leave_out): (u64, &[String]),
    moved: &[MovedFolder],
    drives: &dyn Drives,
    rename: bool,
    mine: Option<&Claim>,
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
    // A half copy an earlier try at this copy left is cleared before copying: the room it takes is free then.
    let mut freed = 0;
    if fs::symlink_metadata(&to).is_ok() {
        match own_room(&to, from, moved, mine, drives, stop) {
            Ok(Some(bytes)) => freed = bytes,
            Ok(None) => return Some(CantMove::TargetThere(to.to_string_lossy().into_owned())),
            Err(why) => return Some(why),
        }
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
    // Counted again here, as the drive it goes to keeps files; `bytes` (measured earlier) only when it can't be. All
    // of it, when a half copy there goes first. Counted even when the drive's room can't be told: it also finds a link
    // to a file that's only online, and a link the drive can't hold.
    let tree = Tree {
        piece: drives.piece(Path::new(destination)),
        links: drives.holds_links(Path::new(destination)),
        leave_out,
        as_new: freed > 0,
        ..Tree::new(from_path, &to, &to)
    };
    let needs = match copy_tree(&tree, Fill::Count, &|_| {}, stop) {
        Ok(counted) => counted.bytes,
        Err(why @ (CantMove::Stopped | CantMove::LinkOnlineOnly { .. } | CantMove::NoLinksThere(_))) => {
            return Some(why);
        }
        Err(_) => bytes,
    };
    let free = drives.free(Path::new(destination))?.saturating_add(freed);
    (free < needs.saturating_add(HEADROOM)).then_some(CantMove::NoRoom { free, needs })
}

/// Copies the folder to the other drive and checks every file arrived whole, then swaps the original for a link to the
/// copy. If anything goes wrong before the swap, the half-made copy is removed and nothing else has changed; if the app
/// or the computer stops meanwhile, the copy's marker says it's a half copy, and the next try clears it first (when
/// nothing uses it and it holds only copies: [`crate::half_copy`]). `kind` and `key` say whose folder it is;
/// `shared_with`, the other plugins it's also the folder of. `progress` hears the bytes copied so far. Call it away
/// from the window's thread.
#[allow(clippy::too_many_arguments)]
pub fn move_folder(
    from: &str,
    destination: &str,
    bytes: u64,
    (key_of, kind, shared_with): (&str, SpaceKind, &[String]),
    drives: &dyn Drives,
    already_moved: &[MovedFolder],
    protected: &[String],
    progress: &(dyn Fn(u64) + Sync),
    stop: &AtomicBool,
) -> Result<MovedFolder, CantMove> {
    let from = from.trim_end_matches(['/', '\\']).to_owned();
    // Before anything is made there (the claim, a half copy cleared).
    if protected_folder(&from, protected) {
        return Err(CantMove::Protected);
    }
    // The app or the computer stopped in the moment the folder was called "… (checking)": it gets its name back.
    put_back_checked(&from)?;
    let to = target(&from, destination);
    // Nothing else in the app makes or clears a copy there until this one is in use.
    let mine = claim(&to).ok_or_else(|| busy_now(&to))?;
    // A half copy an earlier try left, cut off part way (the app or the computer stopped), goes first.
    clear_own(&to, &from, already_moved, &mine, drives, stop)?;
    if let Some(why) = problem_with(&from, destination, bytes, drives, (already_moved, protected), Some(&mine), stop) {
        return Err(why);
    }
    // Find out before copying whether Windows lets the folder be renamed, rather than after an hour of copying. One it
    // guards is checked as administrator now and swapped as administrator after the copy: Windows asks both times.
    check_rename_or_ask(&from, drives.admin())?;
    let leave_out = left_out(&from, already_moved);
    let tree =
        Tree { leave_out: &leave_out, links_lead: Some(Path::new(&from)), ..Tree::new(Path::new(&from), &to, &to) };
    copy_fresh(&tree, progress, stop)?;
    // Still the real folder that was copied: one that turned into a link meanwhile would be set aside as the old copy,
    // a link, with the plugin's files somewhere else. One that can't be looked at at all now (its drive went just as
    // the copy was done, say) leaves the copy as the only one that may be reached: it stays, as one stopped part way
    // does.
    if !real_folder(&from) {
        if fs::symlink_metadata(&from).is_err() {
            let from = Path::new(&from);
            return Err(stays_while_gone("The folder hasn't moved, though all of it was copied.", &to, from, from));
        }
        remove_copy(&to);
        return Err(CantMove::Failed(format!("{from} isn't a real folder now, so it was left alone.")));
    }
    going_into_use(&to).then_some(()).ok_or_else(|| not_noted(&to))?;
    let aside = format!("{from}{OLD_SUFFIX}");
    if let Err(why) = swap_or_ask(&from, &to, &aside, drives.admin()) {
        // The copy goes only while nothing leads into it and the original is still there (where it was, or set
        // aside): otherwise it may be the copy the plugin uses, or the only one.
        if !is_link(Path::new(&from)) && (real_folder(&from) || real_folder(&aside)) {
            remove_copy(&to);
        }
        return Err(why);
    }
    // In use now: it's no half copy. A marker that won't come out stays as a leftover, and nothing more.
    unmark(&to);
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
pub(crate) fn csharp_time(now: chrono::DateTime<chrono::Local>) -> String {
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
    /// Links in the old copy where the other drive has a file or folder now, or a link leading somewhere else (the
    /// plugin pointed it elsewhere since, or just before the move's swap), and files and folders in it where the other
    /// drive has a link now, as paths inside the folder: left as they were, for you to look at. So is the link of a
    /// folder moved from inside it before that Windows wouldn't let be pointed where that folder is now.
    pub links_left: Vec<String>,
}

/// Undoes a move: the link goes and the old copy takes its place again, with what changed on the other drive since the
/// move copied into it first (presets the plugin wrote through the link live only there). The old copy's own newer
/// files are kept aside, never replaced, and its links stay as they are: one that leads somewhere else than the other
/// drive's is named ([`MovedBack::links_left`]), as is a file or folder of its where the other drive has a link now
/// (it stays too). But the links of folders moved from inside it before (`all` is every moved folder) are the app's
/// own: once both passes are done, one whose twin on the other drive leads where its folder is now is pointed there
/// too (the old copy's may lead where it was before the folder the moved folders live in moved). Once the old copy is
/// deleted, the folder is copied back instead.
/// Either way only with room to spare on this drive, and only while the link still leads to the copy the move made.
/// Close the DAW first: what it saves meanwhile is caught by a second pass, but not after the swap. Stopped or failing
/// part way, it says what's where ([`CantMove::LeftHalfway`]); the copy it makes when the old copy is gone holds a
/// marker until it's in place, so one the app or the computer stopped is cleared by the next try
/// ([`crate::half_copy`]). Once it returns `Ok`, the move is over: the app lists the folders of kept files beside it
/// ([`crate::after_move::kept_beside`], which finds those a stopped try left too) as folders left behind, then drops
/// its record; the copy on the other drive is left for you to remove. Call it away from the window's thread.
pub fn move_back(
    moved: &MovedFolder,
    all: &[MovedFolder],
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
    // A marker left in the copy (the app stopped between a swap and taking it out) would make it look like a half
    // copy once the move is over, and it's left for you.
    unmark(to);
    if moved.has_old_copy() {
        let kept_in = PathBuf::from(fresh_name(from, KEPT_SUFFIX));
        let changed = AtomicBool::new(false);
        // Where the copy really is as moving back begins: both passes copy from there, or stop ([`Tree::real_from`]).
        let real_from = fs::canonicalize(to).map_err(|e| failed(to, &e))?;
        let tree = Tree {
            keep_in: Some(&kept_in),
            changed: Some(&changed),
            real_from: Some(&real_from),
            ..Tree::new(to, Path::new(&moved.old_copy), settled)
        };
        room_for(&tree, here, drives, stop)?;
        // From here on the old copy changes: a stop or a failure once it has says so, and where its own newer files
        // are. Before that, nothing was moved.
        let partly = |why: CantMove| match why {
            CantMove::LeftHalfway(_) => why,
            other if !changed.load(Ordering::Relaxed) && !kept_in.exists() => other,
            other => {
                let kept = if kept_in.exists() {
                    format!(", and its own newer files are kept in {}", text(&kept_in))
                } else {
                    String::new()
                };
                CantMove::LeftHalfway(format!(
                    "{} The old copy ({}) was partly brought up to date from {}{kept}. The link and {} are as they \
                     were, so it's safe to try again once the DAW is closed.",
                    part_way(&other),
                    moved.old_copy,
                    moved.to,
                    moved.to
                ))
            }
        };
        // New files, and files that differ, go into the old copy; nothing in it is removed.
        let first = copy_tree(&Tree { last: false, ..tree }, Fill::Freshen, progress, stop).map_err(partly)?;
        // What the plugin saved through the link meanwhile. What the first pass wrote isn't yours: it isn't kept.
        let mut ours = first.written.clone();
        ours.sort();
        let again = copy_tree(
            &Tree { ours: &ours, as_files: &first.as_files, last: true, ..tree },
            Fill::Freshen,
            &|n| progress(first.bytes.saturating_add(n)),
            stop,
        )
        .map_err(partly)?;
        write_through(&[&first, &again], stop).map_err(partly)?;
        let mut links_left = [first.links_left, again.links_left].concat();
        // The copying took a while: a plugin update may have put a real folder where the link was meanwhile. That's
        // looked at before the old copy's links are pointed (Windows may ask you about those, for nothing then), and
        // again just before the swap.
        let not_swapped = |found: NotTheLink| {
            let kept = if kept_in.exists() {
                format!(" Its own newer files are kept in {}.", text(&kept_in))
            } else {
                String::new()
            };
            let updated = if changed.load(Ordering::Relaxed) {
                format!(
                    "The old copy ({}) was brought up to date from {}, and stays beside it.",
                    moved.old_copy, moved.to
                )
            } else {
                format!("The old copy ({}) stays beside it, as it was.", moved.old_copy)
            };
            CantMove::LeftHalfway(format!("{} {updated}{kept} {}", found.now, found.files))
        };
        if let Some(found) = not_the_link_now(moved) {
            return Err(not_swapped(found));
        }
        point_inner_links(moved, all, &mut links_left, &changed, drives.admin()).map_err(partly)?;
        if let Some(found) = not_the_link_now(moved) {
            return Err(not_swapped(found));
        }
        unswap_or_ask(from, &moved.old_copy, to, drives.admin()).map_err(partly)?;
        return Ok(MovedBack {
            kept: shown(&[first.kept, again.kept].concat()),
            kept_in: kept_in.to_string_lossy().into_owned(),
            links_left: shown(&links_left),
        });
    }
    let back = format!("{from}{BACK_SUFFIX}");
    let mine = claim(Path::new(&back)).ok_or_else(|| busy_now(Path::new(&back)))?;
    // A half copy an earlier try left, cut off part way (the app or the computer stopped), goes first.
    clear_own(Path::new(&back), &moved.to, std::slice::from_ref(moved), &mine, drives, stop)?;
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
    // The copying took a while: a plugin update may have put a real folder where the link was meanwhile. The copy
    // made to come back is only a copy of what's still on the other drive, so it goes; but not while the other drive
    // can't be reached (its drive went just now, say): then it's the only copy that can be. It stays a half copy then,
    // its marker saying it isn't going into use, which the next try clears first, once the other drive can be reached
    // again ([`clear_own`]).
    let not_swapped = |found: NotTheLink| {
        if found.reached {
            remove_copy(Path::new(&back));
            return CantMove::Failed(format!(
                "{} The copy made to bring it back was removed; {}",
                found.now, found.files
            ));
        }
        not_in_use(Path::new(&back));
        CantMove::LeftHalfway(format!(
            "{} The copy made to bring it back stays beside it, as {back}: it has all the folder's files, as they were \
             when it was made. {}",
            found.now, found.files
        ))
    };
    if let Some(found) = not_the_link_now(moved) {
        return Err(not_swapped(found));
    }
    going_into_use(Path::new(&back)).then_some(()).ok_or_else(|| not_noted(Path::new(&back)))?;
    if let Err(why) = unswap_or_ask(from, &back, to, drives.admin()) {
        // While the link is there (as it was, or put back) or a real folder is (a plugin update put it there just
        // now), nothing leads to the copy made to come back: the swap didn't happen, so it's only a copy, as above.
        // When the link still leads to the other drive, it goes, and `why` says why it didn't swap. Anything else (the
        // link went, and the copy didn't take its name) is said in `why` (what to rename, say), and the copy stays as
        // it is, still noted as going into use.
        if !is_link(Path::new(from)) && !real_folder(from) {
            return Err(why);
        }
        return Err(match not_the_link_now(moved) {
            Some(found) => not_swapped(found),
            None => {
                remove_copy(Path::new(&back));
                why
            }
        });
    }
    // In place now, as the plugin's own folder: it's no half copy.
    unmark(settled);
    Ok(MovedBack { links_left: shown(&[first.links_left, again.links_left].concat()), ..MovedBack::default() })
}

/// Moving back into the old copy, once both passes are done: the links of folders moved from inside it before
/// (Reaktor 6 inside Native Instruments) are the app's own, and the old copy's may lead where such a folder was before
/// the folder the moved folders live in moved (the app points the live ones elsewhere then, never these). Each one the
/// copy's leads where its folder is now is pointed there too, so the plugin finds that folder once the old copy is back,
/// and isn't named in `links_left` (the passes named it, as it led elsewhere than the copy's). It leads to the place the
/// app noted for that folder, never to one read through the copy. Only a link in the old copy itself, with no link on
/// the way, is changed. The ones Windows guards are changed as administrator, Windows asking you once for all of them
/// (`admin`: the app passes [`crate::admin::AsAdmin`]), as when pointing links again. One that still won't change stays
/// as it was, and is named; one that went and couldn't be put back stops moving back, before the old copy is in use,
/// and is named.
fn point_inner_links(
    moved: &MovedFolder,
    all: &[MovedFolder],
    links_left: &mut Vec<PathBuf>,
    changed: &AtomicBool,
    admin: &dyn Admin,
) -> Result<(), CantMove> {
    let (old, copy) = (Path::new(&moved.old_copy), Path::new(&moved.to));
    let mut guarded = Vec::new();
    for (rest, inner) in inner_links(moved, all) {
        let (mine, theirs, now) = (old.join(&rest), copy.join(&rest), PathBuf::from(&inner.to));
        if !in_itself(old, &rest) || !is_link(&mine) || leads_to(&text(&mine), &now) || !leads_to(&text(&theirs), &now)
        {
            continue;
        }
        match crate::after_move::relink(&text(&mine), &now, true) {
            Ok(()) => {
                changed.store(true, Ordering::Relaxed);
                unname(links_left, &rest);
            }
            Err(not) if not.gone => {
                changed.store(true, Ordering::Relaxed);
                return Err(CantMove::Failed(not.why));
            }
            // Changed as administrator, below, with the others Windows guards.
            Err(not) if not.guarded && admin.can_ask() => guarded.push((rest, mine, now)),
            // It stays as it was, and named: the passes named it already, as it leads elsewhere than the copy's.
            Err(_) => name_once(links_left, rest),
        }
    }
    if guarded.is_empty() {
        return Ok(());
    }
    let asked: Vec<(String, PathBuf)> = guarded.iter().map(|(_, mine, now)| (text(mine), now.clone())).collect();
    for ((rest, mine, _), done) in guarded.into_iter().zip(crate::after_move::relink_as_admin(admin, &asked)) {
        match done {
            Ok(()) => {
                changed.store(true, Ordering::Relaxed);
                unname(links_left, &rest);
            }
            // Still there (as it was, or something the plugin put in its place meanwhile): it stays, and is named.
            Err(_) if fs::symlink_metadata(&mine).is_ok() => name_once(links_left, rest),
            Err(why) => {
                changed.store(true, Ordering::Relaxed);
                return Err(CantMove::Failed(why));
            }
        }
    }
    Ok(())
}

/// Names `rest` in what moving back left, once: by its name in the copy, as the passes named it, maybe in other
/// capitals than the move noted.
fn name_once(links_left: &mut Vec<PathBuf>, rest: PathBuf) {
    if !links_left.iter().any(|p| key(&text(p)) == key(&text(&rest))) {
        links_left.push(rest);
    }
}

/// Takes `rest` out of what moving back left, whatever capitals the passes named it in.
fn unname(links_left: &mut Vec<PathBuf>, rest: &Path) {
    links_left.retain(|p| key(&text(p)) != key(&text(rest)));
}

/// Whether `rest` (a path inside `folder`, by names only: never "..") is in `folder` itself: `folder` is a real folder,
/// and so is every one on the way to it ([`not_real_on_the_way`]), so changing what's at `rest` changes nothing
/// anywhere else. Looked at before anything at `rest` is, so a link on the way (to a network drive that's asleep, say)
/// is never followed.
fn in_itself(folder: &Path, rest: &Path) -> bool {
    let parts: Vec<Component> = rest.components().collect();
    fs::symlink_metadata(folder).is_ok_and(|m| m.is_dir())
        && parts.iter().all(|part| matches!(part, Component::Normal(_)))
        && parts.split_last().is_some_and(|(_, up)| not_real_on_the_way(folder, up.iter().copied()).is_none())
}

/// The first folder on the way down `parts` from `folder`, looked at name by name, that isn't a real folder now: a link,
/// a file, or nothing there. Never through a link: each is looked at only once every one above it is a real folder.
fn not_real_on_the_way<'a>(folder: &Path, parts: impl IntoIterator<Item = Component<'a>>) -> Option<PathBuf> {
    let mut at = folder.to_path_buf();
    parts.into_iter().find_map(|part| {
        at.push(part);
        (!fs::symlink_metadata(&at).is_ok_and(|m| m.is_dir())).then(|| at.clone())
    })
}

/// Why the place a moved folder was isn't its link to the copy any more, if it isn't, and then where the folder's files
/// are and what to do (the end of what the app says): a real folder is there (a plugin update may have made it), which
/// linking it again from the Space page puts right; or the copy can't be reached now while the link is still worded as
/// the move made it (the copy's drive went, or a link of yours the copy is leads nowhere), which trying again once it's
/// back does; or the link leads somewhere else (it was pointed at another place); or nothing is there. Only a real
/// folder can be linked again ([`crate::after_move::link_again`]). The copy is said to have all the folder's files only
/// while it can be reached.
fn not_the_link_now(moved: &MovedFolder) -> Option<NotTheLink> {
    if link_reaches(moved) {
        return None;
    }
    let (from, to) = (&moved.from, &moved.to);
    let link = Path::new(from);
    let reached = is_dir(to);
    let files = if reached {
        format!("{to} still has all the folder's files.")
    } else {
        format!("The folder's files on {to} were left as they are, but it can't be reached now.")
    };
    // The link's own words still name the copy (in whatever capitals): it leads nowhere only by the way there.
    let names_copy = || {
        fs::read_link(link)
            .is_ok_and(|words| key(&text(&worded_place(link, &words))) == key(&text(&plainly(&tidy(Path::new(to))))))
    };
    let (now, files) = if real_folder(from) {
        (
            format!(
                "While it was moving back, {from} became a real folder again (a plugin update may have put it there), \
                 so it wasn't swapped."
            ),
            format!("{files} Look at what's in {from} now, then link it again from the Space page."),
        )
    } else if is_link(link) && !reached && names_copy() {
        (
            format!(
                "While it was moving back, {to} couldn't be reached any more (its drive may have gone), so it wasn't \
                 swapped."
            ),
            format!("{files} Once it can be reached again, try again."),
        )
    } else if is_link(link) {
        (
            format!(
                "While it was moving back, the link at {from} was changed to lead somewhere else, so it wasn't swapped."
            ),
            format!("{files} Look at where the link at {from} leads now."),
        )
    } else {
        (format!("While it was moving back, the link at {from} went, so it wasn't swapped."), files)
    };
    Some(NotTheLink { now, files, reached })
}

/// What [`not_the_link_now`] found: why the place a moved folder was isn't its link to the copy any more (`now`), where
/// the folder's files are and what to do (`files`, the end of what the app says), and whether the copy can be reached
/// now. That's looked at once, so what's done with the copy made to come back goes by what's said, also on a drive
/// that's slow to wake up.
struct NotTheLink {
    now: String,
    files: String,
    reached: bool,
}

/// A copy whose marker couldn't be told it's going into use goes (it isn't in use), and nothing has moved: a copy whose
/// marker still says it's being copied mustn't go into use.
pub(crate) fn not_noted(copy: &Path) -> CantMove {
    remove_copy(copy);
    CantMove::Failed(format!("Couldn't note in {} that the copy is going into use.", copy.to_string_lossy()))
}

/// What a stop or a failure part way says first: "Stopped.", or why, without "Nothing was moved." (something was).
pub(crate) fn part_way(why: &CantMove) -> String {
    match why {
        CantMove::Stopped => "Stopped.".to_owned(),
        other => other.text().trim_end_matches("Nothing was moved.").trim_end().to_owned(),
    }
}

/// Whether the drive `here` is on has room for what freshening `tree` copies, and some to spare (when the drive's room
/// can be told). Counting it also finds, before anything changes, what would stop the copy part way: two names that
/// differ only in capitals (when one of them is in the copy already), a file where a folder goes, a link to a file
/// that's only online, a link the drive can't hold.
pub(crate) fn room_for(tree: &Tree, here: &Path, drives: &dyn Drives, stop: &AtomicBool) -> Result<(), CantMove> {
    let counting = Tree { piece: drives.piece(here), links: drives.holds_links(here), ..*tree };
    let needs = copy_tree(&counting, Fill::Count, &|_| {}, stop)?.bytes;
    match drives.free(here) {
        Some(free) if free < needs.saturating_add(HEADROOM) => Err(CantMove::NoRoom { free, needs }),
        _ => Ok(()),
    }
}

/// Copies a folder into a new folder made for it (so a folder that appeared meanwhile is never copied into, or later
/// removed), then a second time for what was saved while it copied (an hour, for a big one), then writes it through to
/// the disk ([`write_through`]). The new folder holds a marker from the moment it's made ([`crate::half_copy`]), which
/// the caller takes out once the copy is in use, so one the app or the computer stops part way is known for the half
/// copy it is. If anything goes wrong, or you press Stop, the new folder goes; but not while nothing can be looked at
/// where the folder it copies really was as it began (moving back, the other drive went part way, say), as it may be
/// the only copy of some of those files then: it stays, and it says so ([`stays_while_gone`]). Returns what each pass
/// did.
pub(crate) fn copy_fresh(
    tree: &Tree,
    progress: &(dyn Fn(u64) + Sync),
    stop: &AtomicBool,
) -> Result<(Copied, Copied), CantMove> {
    // Where the folder really is as the copy begins: both passes copy from there, or stop ([`Tree::real_from`]).
    let real_from = fs::canonicalize(tree.from).map_err(|e| failed(tree.from, &e))?;
    let tree = Tree { real_from: Some(&real_from), ..*tree };
    fs::create_dir(tree.to)
        .map_err(|e| CantMove::Failed(format!("Couldn't make {} ({e}).", tree.to.to_string_lossy())))?;
    if let Err(e) = half_copy::mark(&tree) {
        remove_copy(tree.to);
        return Err(CantMove::Failed(format!("Couldn't start the copy in {} ({e}).", tree.to.to_string_lossy())));
    }
    let copied = copy_tree(&Tree { last: false, ..tree }, Fill::New, progress, stop)
        .and_then(|first| {
            let again = Tree { as_files: &first.as_files, last: true, ..tree };
            copy_tree(&again, Fill::Freshen, &|n| progress(first.bytes.saturating_add(n)), stop)
                .map(|again| (first, again))
        })
        .and_then(|(first, again)| write_through(&[&first, &again], stop).map(|()| (first, again)));
    copied.map_err(|why| {
        // Nothing can be looked at where it really copied from (its drive went part way, say; whatever the system says,
        // as an unplugged drive can say several things): it stays.
        if fs::symlink_metadata(&real_from).is_err() {
            return stays_while_gone(&part_way(&why), tree.to, tree.from, &real_from);
        }
        // Anything else goes: what it copies is still there, or something else is in its place now (a link put there,
        // so what was read may not be its files).
        remove_copy(tree.to);
        why
    })
}

/// What a copy the move made says when it stays because nothing can be looked at where it was copied from (`real`,
/// where `from` really was as the copy began: its drive went, say): what came across may be the only copy of some of
/// those files that can be reached then. It stays, still a half copy, which the next try clears first once that can be
/// reached again ([`half_copy::clear_own`]). `said` is what stopped it.
fn stays_while_gone(said: &str, to: &Path, from: &Path, real: &Path) -> CantMove {
    // `from` still leads somewhere (a link pointed elsewhere meanwhile): it's where it led that went, not `from`.
    let gone = if fs::metadata(from).is_ok() {
        format!("{}, where {} led,", text(&plainly(real)), text(from))
    } else {
        text(from)
    };
    CantMove::LeftHalfway(format!(
        "{said} The copy made so far stays, as {}: {gone} can't be reached now, so it may hold the only copy of some of \
         its files. Once that can be reached again, trying again clears it first.",
        text(to)
    ))
}

/// Writes a copy through to the disk just before it goes into use: every big file its passes wrote
/// ([`Copied::big`]), and the small files they wrote last ([`Copied::recent`]), each once (a file put into a folder of
/// yours was before it went in, unless that couldn't be done then: [`Put::Through`]). Windows keeps a file's bytes in
/// memory a while after it's written, and a power cut just after the swap mustn't leave the plugin using files whose
/// bytes never reached the disk; small files written earlier are taken to be on the disk by then ([`WRITTEN_THROUGH`]).
/// A file that can't be opened for it is left to the system. Spread over the cores of the pool it's called in. Stop is
/// looked at before each file: stopped, it says so, and the copy doesn't go into use. (The progress bar stays full
/// meanwhile: it's seconds, or a minute or two for a big copy to a slow drive.)
pub(crate) fn write_through(passes: &[&Copied], stop: &AtomicBool) -> Result<(), CantMove> {
    let mut files: Vec<&PathBuf> = passes.iter().flat_map(|pass| pass.big.iter().chain(&pass.recent)).collect();
    // A file both passes wrote is written through once.
    files.sort_unstable();
    files.dedup();
    files.par_iter().for_each(|file| {
        if !stop.load(Ordering::Relaxed) {
            flush_file(file);
        }
    });
    if stop.load(Ordering::Relaxed) { Err(CantMove::Stopped) } else { Ok(()) }
}

/// Writes one file through to the disk, and says whether it could. Windows only does that for a program that may
/// write to the file, so a read-only one is let be written to for the moment.
fn flush_file(file: &Path) -> bool {
    #[cfg(windows)]
    {
        let was_read_only = set_read_only(file, false);
        let flushed = fs::File::options().write(true).open(file).and_then(|open| open.sync_all()).is_ok();
        if was_read_only {
            set_read_only(file, true);
        }
        flushed
    }
    #[cfg(not(windows))]
    {
        fs::File::open(file).and_then(|open| open.sync_all()).is_ok()
    }
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

/// The links of folders moved from inside this one before (Reaktor 6 inside Native Instruments), by their paths inside
/// it, with their moves: the app's own, which those moves made.
fn inner_links<'a>(moved: &'a MovedFolder, all: &'a [MovedFolder]) -> Vec<(PathBuf, &'a MovedFolder)> {
    inner_moves(moved, all).filter_map(|m| Some((rest_after(&m.from, &moved.from)?, m))).collect()
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
/// saved there while the move ran, or deleted from the other drive since, and links that lead somewhere else from the
/// old copy than from the copy ([`same_lead`]: the plugin pointed one elsewhere just before the swap, or in the copy
/// since). The old copies of folders moved from inside it before are checked against their own copies; their links in
/// it are the app's own, so any link at their place in the copy counts (one that isn't there is named). Reads both
/// folders: call it away from the window's thread.
pub fn only_in_old_copy(moved: &MovedFolder, all: &[MovedFolder], stop: &AtomicBool) -> Result<Vec<String>, CantMove> {
    old_copy_only(moved, all, false, stop)
}

/// [`only_in_old_copy`], and with `bytes`, also the files that look the same in both (size and time) but whose bytes
/// differ: a copy gone bad on the way (a faulty USB cable, a stick smaller than it says). Reads every such file in
/// both: slow, so only just before the old copy goes.
fn old_copy_only(
    moved: &MovedFolder,
    all: &[MovedFolder],
    bytes: bool,
    stop: &AtomicBool,
) -> Result<Vec<String>, CantMove> {
    let mut only = Vec::new();
    let old = Path::new(&moved.old_copy);
    differences((old, Path::new(&moved.to)), Some(moved), (all, bytes), Path::new(""), &mut only, stop)?;
    only.sort();
    Ok(only)
}

/// What's in `old` but not in `copy`, or newer in `old`, as paths inside it; with `bytes`, also the files that look the
/// same in both but whose bytes differ (slow: only just before `old` goes).
pub(crate) fn only_in(old: &Path, copy: &Path, bytes: bool, stop: &AtomicBool) -> Result<Vec<String>, CantMove> {
    let mut only = Vec::new();
    differences((old, copy), None, (&[], bytes), Path::new(""), &mut only, stop)?;
    only.sort();
    Ok(only)
}

/// Adds what's in `old` but not in `copy`, or newer in `old`, as paths under `shown_as`. With `moved` (whose old copy
/// `old` is, or holds), the old copies of folders moved from inside it before, as they sit in `old`, are each checked
/// against their own copy, and their links in it aren't compared with the copy's: they're the app's own, which it
/// points elsewhere as those folders move on (moving the folder the moved folders live in does), the live ones, never
/// these. Any link at their place in the copy counts; one that isn't there is named.
fn differences(
    (old, copy): (&Path, &Path),
    moved: Option<&MovedFolder>,
    (all, bytes): (&[MovedFolder], bool),
    shown_as: &Path,
    only: &mut Vec<String>,
    stop: &AtomicBool,
) -> Result<(), CantMove> {
    let inner = moved.map_or_else(Vec::new, |moved| held(old, moved, all));
    let skip: Vec<String> = inner.iter().map(|(at, _)| key(&at.to_string_lossy())).collect();
    let links: Vec<String> = moved.map_or_else(Vec::new, |moved| {
        inner_links(moved, all).into_iter().map(|(rest, _)| key(&text(&old.join(rest)))).collect()
    });
    let show = |p: &Path| shown_as.join(p.strip_prefix(old).unwrap_or(p)).to_string_lossy().into_owned();
    // Where the two really are, for where links in them lead ([`same_lead`]), plainly.
    let roots = [fs::canonicalize(old).ok().map(|r| plainly(&r)), fs::canonicalize(copy).ok().map(|r| plainly(&r))];
    let mut folders = vec![(old.to_path_buf(), copy.to_path_buf())];
    while let Some((old_dir, copy_dir)) = folders.pop() {
        let top = old_dir.as_path() == old;
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
            let name = entry.file_name();
            // A half copy's marker (left at the top when moving back couldn't take it out) is the app's, not yours.
            if skip.contains(&key(&path.to_string_lossy())) || top && name == MARKER {
                continue;
            }
            let there = copy_dir.join(&name);
            let Ok(kind) = entry.file_type() else {
                only.push(show(&path));
                continue;
            };
            // Nor is a temporary file a copy cut off part way left (moving back writes them into the old copy), nor a
            // pipe or a socket ([`special`]): they're never copied on.
            if kind.is_file() && own_name(&name.to_string_lossy()).is_some() || special(&kind) {
                continue;
            }
            let in_copy = if kind.is_symlink() {
                match fs::symlink_metadata(&there) {
                    // A link there too, leading to the same place. One the plugin pointed elsewhere just before the
                    // swap, after the copy had made it again, leads there only from the old copy. So does one pointed
                    // elsewhere in the copy since, through the folder's link: which of the two was pointed elsewhere
                    // last can't be told (the app makes links again with new times: moving the folder the moved
                    // folders live in does), so it's named, to look at before the old copy goes. But the link of a
                    // folder moved from inside it before is the app's own, which it points elsewhere as that folder
                    // moves on (the live one, never the old copy's): any link there counts.
                    Ok(m) if m.file_type().is_symlink() => {
                        links.contains(&key(&path.to_string_lossy())) || same_lead(&path, &there, &roots)
                    }
                    // Something else there: the plugin saved over its link since.
                    Ok(_) => true,
                    // A link to a pipe or a socket is passed over too, by a copy that couldn't make it again.
                    Err(_) => fs::metadata(&path).is_ok_and(|m| special(&m.file_type())),
                }
            } else if kind.is_dir() {
                let is_dir = fs::metadata(&there).is_ok_and(|m| m.is_dir());
                if is_dir {
                    folders.push((path.clone(), there));
                }
                is_dir
            } else {
                match (entry.metadata(), fs::metadata(&there)) {
                    (Ok(mine), Ok(copy)) => {
                        copy.is_file()
                            && (newer(&copy, &mine)
                                || same_file(&mine, &copy) && (!bytes || same_inside(&path, &there, stop)?))
                    }
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
        differences((&at, Path::new(&m.to)), Some(m), (all, bytes), &shown_as.join(rest), only, stop)?;
    }
    Ok(())
}

/// Whether two links lead to the same place: worded with the same whole path, or by where they lead
/// ([`where_it_leads`]); two whose places can't be told (a link that can't be read) aren't the same. A place inside
/// either folder of `roots` (where the two folders really are, written [`plainly`]: the old copy and the copy, say)
/// counts by its path inside it: an old copy's link into the folder leads into the old copy, or by the folder's own
/// path into the copy, as the copy's does.
fn same_lead(a: &Path, b: &Path, roots: &[Option<PathBuf>]) -> bool {
    let (x, y) = (fs::read_link(a).ok(), fs::read_link(b).ok());
    // Worded with the same whole path, they lead to the same place wherever they are, without asking where (a network
    // drive that's asleep can take a while to say). A copy makes a link again worded with a whole path ([`copy_link`]).
    if let (Some(x), Some(y)) = (&x, &y)
        && x.is_absolute()
        && plainly(x) == plainly(y)
    {
        return true;
    }
    // Each place plainly (without the `\\?\` Windows starts one it looked up with), so one told by its words alone (a
    // network drive that doesn't answer) is compared with one looked up, and with the roots, as it would be written.
    let place = |link: &Path, words: Option<&Path>| {
        where_it_leads(link, words).map(|real| {
            let real = plainly(&real);
            let inside = roots.iter().flatten().find_map(|root| real.strip_prefix(root).ok().map(Path::to_path_buf));
            inside.map_or((false, real), |rest| (true, rest))
        })
    };
    // One whose place can't be told isn't the same as any: the other isn't asked about then.
    place(a, x.as_deref()).is_some_and(|p| place(b, y.as_deref()).is_some_and(|q| q == p))
}

/// Where a link leads: where it really does ([`fs::canonicalize`]), or, leading nowhere now (a drive that isn't plugged
/// in), where it would by its wording (`words`, as [`fs::read_link`] gave it: [`worded_place`]), as far as that place
/// is there, where it really is, and the rest by its words. So two that lead nowhere now are told apart by where they
/// would lead. Where it leads can't always be asked (a network drive that doesn't answer): then it's by its words
/// alone. So the system is asked as few times as can be, as each can take a while to say no: the place is looked up
/// from the top down, as far as it's there.
fn where_it_leads(link: &Path, words: Option<&Path>) -> Option<PathBuf> {
    let nowhere = match fs::canonicalize(link) {
        Ok(real) => return Some(real),
        Err(e) => e.kind() == io::ErrorKind::NotFound,
    };
    let place = worded_place(link, words?);
    if !nowhere {
        return Some(place);
    }
    let mut there = None;
    for up in place.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let Ok(real) = fs::canonicalize(up) else { break };
        there = Some((real, up));
    }
    Some(there.and_then(|(real, up)| Some(real.join(place.strip_prefix(up).ok()?))).unwrap_or(place))
}

/// Whether two files that look the same (size and time) hold the same bytes, read side by side in pieces, stopping when
/// you press Stop. A OneDrive file that's only online, on either side, isn't read (that would download it): OneDrive
/// keeps it, so it counts as the same. One that can't be read (a program has it open) counts as not the same, so the
/// old copy stays.
fn same_inside(a: &Path, b: &Path, stop: &AtomicBool) -> Result<bool, CantMove> {
    let online = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| is_online_only(&m));
    if online(a) || online(b) {
        return Ok(true);
    }
    let (Ok(mut fa), Ok(mut fb)) = (fs::File::open(a), fs::File::open(b)) else { return Ok(false) };
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(CantMove::Stopped);
        }
        let Ok(n) = fa.read(&mut ba) else { return Ok(false) };
        if n == 0 {
            return Ok(fb.read(&mut bb).is_ok_and(|m| m == 0));
        }
        let (Some(want), Some(got)) = (ba.get(..n), bb.get_mut(..n)) else { return Ok(false) };
        if fb.read_exact(got).is_err() || want != &*got {
            return Ok(false);
        }
    }
}

/// Sends the old copy to the Recycle Bin, once [`old_copy_can_go`] says it may, and the old copies of folders moved
/// from inside it before can go too (their links reach their copies). Unless you're `sure`, also only when nothing is
/// only in the old copy ([`only_in_old_copy`]): the app shows that list first. `all` is every moved folder.
/// [`crate::clean::undo`] puts it back.
pub fn remove_old_copy(moved: &MovedFolder, all: &[MovedFolder], sure: bool, bin: &dyn Bin, stop: &AtomicBool) -> Done {
    crate::old_copies::send_one(
        &moved.old_copy,
        moved.bytes,
        crate::old_copies::Going::OldCopy(moved),
        all,
        sure,
        bin,
        stop,
    )
}

/// Why it stays when checking what's only in it was stopped part way.
pub(crate) const UNCHECKED: &str = "Stopped before it was all checked, so it stays.";

/// Why the old copy can't go now, checked just before it goes: [`old_copy_stays`], then, unless you're `sure`, whether
/// anything is only in it, newer there or not the same as on the other drive (what the files hold is compared too, so
/// it reads every file that looks the same in both: slow). `all` is every moved folder.
pub(crate) fn old_copy_check(
    moved: &MovedFolder,
    all: &[MovedFolder],
    sure: bool,
    stop: &AtomicBool,
) -> Result<(), String> {
    if let Some(why) = old_copy_stays(moved, all) {
        return Err(why);
    }
    if sure {
        return Ok(());
    }
    match old_copy_only(moved, all, true, stop) {
        Err(_) => Err(UNCHECKED.to_owned()),
        Ok(only) if !only.is_empty() => Err(format!(
            "Some of it is only in the old copy, newer there, or not the same as on the other drive: {}. It may have \
             been saved while the move ran, deleted from the other drive since, gone bad on the way, or open in a \
             program now. Look before deleting it.",
            listed(&only)
        )),
        Ok(_) => Ok(()),
    }
}

/// Why the old copy can't go now, when it can't: it's a link now, it's the only copy the plugin could use
/// ([`old_copy_can_go`]), or it holds the old copy of a folder moved from inside it before whose link doesn't reach its
/// copy. `all` is every moved folder. What's only in it is looked at when it goes ([`remove_old_copy`]).
pub fn old_copy_stays(moved: &MovedFolder, all: &[MovedFolder]) -> Option<String> {
    if is_link(Path::new(&moved.old_copy)) {
        return Some(
            "It's a link now, not the folder the move set aside, so it stays, and so does where it leads.".to_owned(),
        );
    }
    if leads_into(moved) {
        return Some(
            "The link leads into it (or the copy is in it), so it's what the plugin uses: it stays.".to_owned(),
        );
    }
    if !old_copy_can_go(moved) {
        return Some(
            "It's the only copy the plugin could use now (the link or the copy on the other drive isn't as the move \
             left it), so it stays."
                .to_owned(),
        );
    }
    let mut inner = Vec::new();
    all_held(Path::new(&moved.old_copy), moved, all, &mut inner);
    inner.into_iter().find(|m| !link_reaches(m)).map(|lost| {
        format!(
            "It holds the old copy of {}, whose link doesn't lead to {} now, so it stays.",
            file_name(&lost.from),
            lost.to
        )
    })
}

/// Whether the moved folder's link leads into its old copy (pointed back at it once the other drive went, say), or its
/// copy is in the old copy or holds it: then the old copy is what the plugin uses.
pub(crate) fn leads_into(moved: &MovedFolder) -> bool {
    let Some(old) = real_path(&moved.old_copy) else { return false };
    let link_in = real_path(&moved.from).is_some_and(|f| is_inside(&f, &old));
    let copy_in = real_path(&moved.to).is_some_and(|t| is_inside(&t, &old) || is_inside(&old, &t));
    link_in || copy_in
}

/// Whether Windows said no because of rights (access denied), rather than something having the folder open.
pub(crate) fn denied(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied
}

/// [`check_rename`], as administrator when Windows guards the folder and `admin` can ask (Windows asks you then, and
/// again for the swap after the copy). So a folder even the administrator can't rename, because the DAW has a file in
/// it open, is found before copying rather than after. Unlike the swap, what's there afterwards can't tell "renamed and
/// back" from "never renamed": the folder must be as it was AND the step must say it renamed it and back.
pub(crate) fn check_rename_or_ask(from: &str, admin: &dyn Admin) -> Result<(), CantMove> {
    match check_rename(from) {
        Err(CantMove::NeedsAdmin) if admin.can_ask() => {}
        other => return other,
    }
    let check = format!("{from}{CHECK_SUFFIX}");
    let said = ask_for(admin, Step::Check, &[vec![real_spot(from)]]);
    // The folder is where it was: whatever is called "… (checking)" now (if anything) isn't it.
    if real_folder(from) {
        return match said {
            Ok(outcomes) if outcomes.first() == Some(&Outcome::Done) => Ok(()),
            other => Err(as_it_was(other, "The administrator step finished, but couldn't check the folder.")),
        };
    }
    // Gone, or a link now, and nothing took the check's name: whatever happened, the step didn't do it.
    if not_there(Path::new(&check)) {
        return Err(as_it_was(said, &format!("{from} isn't a real folder now, so it was left alone.")));
    }
    Err(not_as_expected(
        said,
        format!(
            "After the administrator's check, {from} should be as it was, but it isn't quite: look at it, and for \
             \"{}\" beside it, before trying again. Nothing was copied.",
            file_name(&check)
        ),
    ))
}

/// Renames the folder and straight back, to learn whether the swap will work.
pub(crate) fn check_rename(from: &str) -> Result<(), CantMove> {
    let check = format!("{from}{CHECK_SUFFIX}");
    put_back_checked(from)?;
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

/// Gives a folder its name back when the app or the computer stopped in the moment [`check_rename`] had it called
/// "… (checking)": nothing is at its own name, and a real folder (not a link) has that name. `Ok(true)` when it was
/// renamed back; an error when it couldn't be (it's left as it is, and said where).
pub fn put_back_checked(from: &str) -> Result<bool, CantMove> {
    let from = from.trim_end_matches(['/', '\\']);
    let check = format!("{from}{CHECK_SUFFIX}");
    if fs::symlink_metadata(from).is_ok() || !real_folder(&check) {
        return Ok(false);
    }
    rename_back(&check, from).map(|()| true).map_err(|e| {
        CantMove::LeftHalfway(format!(
            "The folder is called \"{}\" since an earlier try was cut off, and Windows wouldn't rename it back ({e}). \
             Rename it to \"{}\" yourself.",
            file_name(&check),
            file_name(from)
        ))
    })
}

/// Renames a folder or a file, trying again for a few seconds while something (a virus checker) looks in.
fn rename_back(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let (from, to) = (from.as_ref(), to.as_ref());
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
    let kind = fs::symlink_metadata(at)?.file_type();
    if !kind.is_symlink() {
        return Err(io::Error::other("not a link"));
    }
    // A junction or a folder link is removed as a folder on Windows, a file link as a file; a symbolic link as a file
    // elsewhere.
    if folder_link(&kind) { fs::remove_dir(at) } else { fs::remove_file(at) }
}

/// Whether a link is one Windows removes as a folder: a junction, or a folder link. (Elsewhere every link is removed
/// as a file.)
fn folder_link(kind: &fs::FileType) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        kind.is_symlink_dir()
    }
    #[cfg(not(windows))]
    {
        let _ = kind;
        false
    }
}

/// Removes a copy the move made itself, which never went into use: the move stopped or failed before then, or its swap
/// didn't happen (nothing leads to it). Never a link, never the original. Its marker says first that it isn't going
/// into use (when it said it was), and goes last, so a copy that can't all go (a file in it is open) is still known for
/// the half copy it is, and cleared by the next try, as one cut off part way is. All of it goes, Stop or not: it's what
/// a stopped move leaves.
pub(crate) fn remove_copy(path: &Path) {
    if fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) && !is_link(path) {
        not_in_use(path);
        let _ = remove_marked(path, &AtomicBool::new(false));
    }
}

/// How [`copy_tree`] treats what's in the copy already.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fill {
    /// The copy was just made, so anything there already is a second file of the same name but for capitals, on a
    /// drive that doesn't tell them apart: the copy stops.
    New,
    /// The copy has most of it: files that are new, or differ in size or time, are copied, each under a temporary name
    /// first; nothing is removed, and links there stay as they are, but in a copy the move made, where the folder's
    /// link is worded otherwise by now, or the folder has a file or a folder in its place: the link the pass before
    /// made goes then, and what's there is copied as it is now ([`copy_tree`]).
    Freshen,
    /// Nothing is copied: only the bytes [`Fill::Freshen`] would copy are added up.
    Count,
}

/// A folder to copy, and where.
pub(crate) struct Tree<'a> {
    pub(crate) from: &'a Path,
    pub(crate) to: &'a Path,
    /// Where `to` ends up once everything's done: links leading inside `from` lead to the same place inside it (unless
    /// `links_lead`).
    pub(crate) settled: &'a Path,
    /// Where links leading inside `from` lead in the copy, when not `settled`: a move keeps them on `from` itself,
    /// which its link keeps reaching wherever the copy goes later (moved home, moved back), as the C# app did.
    pub(crate) links_lead: Option<&'a Path>,
    /// Where files in `to` that are newer than the ones replacing them go first (by their paths inside `to`), when
    /// `to` is yours; without it they're replaced (a copy the move made).
    pub(crate) keep_in: Option<&'a Path>,
    /// With `keep_in`: every file in `to` that's replaced is kept there first, not only newer ones.
    pub(crate) keep_all: bool,
    /// With `keep_in`: files an earlier pass wrote into `to`, and links it made there (paths inside it, sorted).
    /// They're the copy's own work, not yours, so they're replaced without being kept: such a link is made again where
    /// the folder's is worded otherwise by then, or goes for the file or folder the folder has in its place, as in a
    /// copy the move made.
    pub(crate) ours: &'a [PathBuf],
    /// Folders not copied (keys).
    pub(crate) leave_out: &'a [String],
    /// Counting: the size of the pieces the drive it goes to keeps files in. Each file counts as a whole number of
    /// them, and each new folder as one, so a folder of many small files isn't taken for smaller than it'll be there.
    pub(crate) piece: u64,
    /// Counting: as for a copy not made yet, whatever is at `to` now (a half copy, cleared before copying).
    pub(crate) as_new: bool,
    /// Counting: whether the drive it goes to holds links ([`Drives::holds_links`]). Where it doesn't, a link inside
    /// that would be made again as a link stops the count, and a file link counts as the file it leads to.
    pub(crate) links: bool,
    /// The last pass before the copy goes into use: nothing looks at the folder again, so a file that turned into a
    /// folder or a link just as it was to be copied stops the copy, rather than being left to the next pass
    /// ([`as_now`]). Yes unless it's said: only a pass that another one follows says it isn't.
    pub(crate) last: bool,
    /// For a copy made in passes: where `from` really was as the first one began ([`fs::canonicalize`]). Each pass
    /// stops before it copies anything once `from` leads somewhere else (it turned into a link meanwhile), so no pass
    /// copies what isn't the plugin's into a copy another pass began.
    pub(crate) real_from: Option<&'a Path>,
    /// File links the pass before copied as the file they lead to, as they couldn't be made again (paths inside `to`,
    /// sorted: [`Copied::as_files`]). The file there is theirs, so it's brought up to date from the file the link
    /// leads to now; at another link, a file or folder in a copy the move made means the link was a real one when the
    /// pass before copied it.
    pub(crate) as_files: &'a [PathBuf],
    /// Set once anything in `to` has changed: a folder or a link made, or a file put in (a temporary one that goes
    /// again isn't a change). So moving back can say whether the old copy changed before it stopped.
    pub(crate) changed: Option<&'a AtomicBool>,
}

impl<'a> Tree<'a> {
    /// `from` copied into `to`, which ends up as `settled`, in one pass; nothing kept aside or left out.
    pub(crate) fn new(from: &'a Path, to: &'a Path, settled: &'a Path) -> Self {
        Tree {
            from,
            to,
            settled,
            links_lead: None,
            keep_in: None,
            keep_all: false,
            ours: &[],
            leave_out: &[],
            piece: 0,
            as_new: false,
            links: true,
            last: true,
            real_from: None,
            as_files: &[],
            changed: None,
        }
    }

    /// Notes that something in `to` has changed ([`Tree::changed`]).
    fn has_changed(&self) {
        if let Some(changed) = self.changed {
            changed.store(true, Ordering::Relaxed);
        }
    }
}

/// What a copy did: bytes copied (or that would be, counting), and what it kept aside or left (paths inside the copy).
#[derive(Default)]
pub(crate) struct Copied {
    pub(crate) bytes: u64,
    pub(crate) kept: Vec<PathBuf>,
    pub(crate) links_left: Vec<PathBuf>,
    /// With `keep_in`, freshening: the files it wrote and the links it made (paths inside the copy), for
    /// [`Tree::ours`] in the next pass.
    pub(crate) written: Vec<PathBuf>,
    /// File links it copied as the file they lead to (paths inside the copy, sorted), for [`Tree::as_files`] in the
    /// next pass.
    pub(crate) as_files: Vec<PathBuf>,
    /// The small files it wrote in its last [`WRITTEN_THROUGH`], at most [`THROUGH_MOST`] of them: written through to
    /// the disk before the copy goes into use ([`write_through`]). Not those put into a folder of yours, written
    /// through before they went in ([`Put::Through`]), but for one that couldn't be then.
    pub(crate) recent: Vec<PathBuf>,
    /// Every big file it wrote ([`IN_PIECES_FROM`] or more), as for [`Copied::recent`]: they hold most of what a move
    /// copies, so they're all written through to the disk before the copy goes into use, however long ago they were
    /// written.
    pub(crate) big: Vec<PathBuf>,
}

/// Copies a folder into another one, file by file, keeping each file's time and checking its size once written. A link
/// inside is made again as a link, never followed. A file or folder that goes while it's read (a plugin's lock file)
/// is passed over ([`went`]), and so is a pipe or a socket ([`special`]). One file at a time: a move usually goes to a
/// spinning drive, where several at once is slower.
pub(crate) fn copy_tree(
    tree: &Tree,
    fill: Fill,
    progress: &(dyn Fn(u64) + Sync),
    stop: &AtomicBool,
) -> Result<Copied, CantMove> {
    let source_real = fs::canonicalize(tree.from).map_err(|e| failed(tree.from, &e))?;
    // The folder itself turned into a link since the first pass began (or, a link itself, leads somewhere else now):
    // nothing is copied from where it leads now into a copy of what it was. Renamed in other capitals only, on a drive
    // that doesn't tell them apart, it's the same folder ([`still_same_place`]).
    if tree.real_from.is_some_and(|real| !still_same_place(&source_real, real)) {
        return Err(changing(tree.from, "turned into a link or led somewhere else"));
    }
    // A folder that turned into a link (or a file) as the copy went, one above it that did, or the folder itself
    // pointed somewhere else (moving back, the copy can be a link of yours), is never read through: what's there now
    // isn't the plugin's ([`led_elsewhere`]). Its folder in the copy is made already, so the copy can't take it as it
    // is now, and it stops, whichever pass it is. Counting only adds up: it doesn't look.
    let from_was_link = is_link(tree.from);
    let still_real =
        |dir: &Path| match (fill != Fill::Count).then(|| led_elsewhere(dir, tree, &source_real, from_was_link)) {
            Some(Some(why)) => Err(why),
            _ => Ok(()),
        };
    // Where the folder and the copy really are, for where links at the same place in both lead, freshening a folder of
    // yours ([`same_lead`]), plainly. Looked up once a pass: the copy's drive is awake anyway, as each name is looked for
    // in it.
    let roots =
        [Some(plainly(&source_real)), tree.keep_in.and_then(|_| fs::canonicalize(tree.to).ok()).map(|r| plainly(&r))];
    let inside = |copy: &Path| copy.strip_prefix(tree.to).unwrap_or(copy).to_path_buf();
    let mut copied = Copied::default();
    let mut recent: VecDeque<(Instant, PathBuf)> = VecDeque::new();
    let mut waiting = Waiting::default();
    // Whether a file link can be made again where the copy goes, once it's asked: the drive holds links, and Windows
    // makes file links for the app ([`makes_file_links`]).
    let mut file_links = None;
    // Counting for a copy not made yet: the folder it's made in takes a piece of the drive too.
    if fill == Fill::Count && (tree.as_new || fs::symlink_metadata(tree.to).is_err()) {
        copied.bytes = tree.piece;
    }
    let mut folders = vec![(tree.from.to_path_buf(), tree.to.to_path_buf())];
    while let Some((source_dir, copy_dir)) = folders.pop() {
        let top = source_dir.as_path() == tree.from;
        // Not read through a link: it, and every folder above it, is still a real folder as its turn comes.
        still_real(&source_dir)?;
        // Listed once. (A file's size and time in the listing can be old by the time its turn comes: it's looked at
        // again just before it's copied, [`as_now`].)
        let listing = match fs::read_dir(&source_dir) {
            Ok(listing) => listing,
            Err(_) if went(&source_dir, tree) => continue,
            Err(e) => return Err(failed(&source_dir, &e)),
        };
        // Counting for a copy not made yet: nothing is there, so the other drive isn't asked about each name.
        let copy_dir_there = fill != Fill::Count || !tree.as_new && fs::symlink_metadata(&copy_dir).is_ok();
        // Where something may be in the copy already (a copy just made has nothing else in it), the names here that
        // differ only in capitals are found first, so the folder is listed whole before anything is copied from it;
        // otherwise it's listed as it's copied. The names in the copy's folder, capitals and all, are read once one of
        // them is asked about.
        let (entries, twins): (Box<dyn Iterator<Item = io::Result<fs::DirEntry>>>, _) =
            if fill != Fill::New && copy_dir_there {
                let whole = listed_whole(listing, &source_dir, tree, stop)?;
                let twins = twins(&whole);
                (Box::new(whole.into_iter().map(Ok)), twins)
            } else {
                (Box::new(listing), HashSet::new())
            };
        let mut names_there = None;
        for entry in entries {
            if stop.load(Ordering::Relaxed) {
                return Err(CantMove::Stopped);
            }
            let entry = match entry {
                Ok(entry) => entry,
                // The folder went while it was read (a plugin's cache, cleared): the rest of it is passed over.
                Err(_) if went(&source_dir, tree) => break,
                Err(e) => return Err(failed(&source_dir, &e)),
            };
            let source = entry.path();
            let name = entry.file_name();
            // A half copy's marker, left in a copy that's in use now, is never copied on (into the old copy, say).
            if tree.leave_out.contains(&key(&source.to_string_lossy())) || top && name == MARKER {
                continue;
            }
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) if went(&source, tree) => continue,
                Err(e) => return Err(failed(&source, &e)),
            };
            // Nor is a temporary file an earlier try left, cut off before it went in (the copy on the other drive can
            // hold one): it isn't the plugin's. Nor a pipe or a socket ([`special`]).
            if kind.is_file() && own_name(&name.to_string_lossy()).is_some() || special(&kind) {
                continue;
            }
            let copy = copy_dir.join(&name);
            // One that would meet a file waiting to go in (a name that differs only in capitals, on a drive that
            // doesn't tell them apart) waits until those are in, so it finds what's there then. (Those read from this
            // folder go in only while it's still a real folder once they're read; those from folders before were
            // looked at as those were done, below.)
            if waiting.meets(&copy) {
                still_real(&source_dir)?;
                waiting.put_in(&mut copied, &mut recent, tree, stop)?;
            }
            let mut there = if copy_dir_there { fs::symlink_metadata(&copy).ok() } else { None };
            // Two names in a folder that differ only in capitals can't both go where the drive finds something for one
            // under another name: it doesn't tell them apart. (A copy just made has nothing else in it.)
            let twin = !twins.is_empty() && twins.contains(&key(&name.to_string_lossy()));
            if there.is_some() && (fill == Fill::New || twin && !named_in(&copy_dir, &mut names_there, &name)) {
                return Err(CantMove::Failed(format!(
                    "{} has a twin whose name differs only in capitals, and the drive it goes to doesn't tell them \
                     apart: rename one of them.",
                    source.to_string_lossy()
                )));
            }
            // A link there stays as it is but in a copy the move made: nothing is written through it (it could lead
            // into another plugin's folder).
            if there.as_ref().is_some_and(|m| m.file_type().is_symlink()) {
                // In a copy the move made, the link is the move's own, and the copy is to be as the folder is: where
                // the link the pass before made isn't worded as the folder's would be now (the plugin pointed it
                // elsewhere), or the folder has a file or a folder in its place (a plugin saved over its link), it
                // goes, and what's there now is copied. Where what the folder had there went just now (a link
                // deleted to be made again, say), it stays: nothing is removed, as for a file that goes.
                let own_goes = || {
                    let as_it_was = kind.is_symlink() && made_as_worded(&source, &copy, tree, &source_real);
                    !as_it_was && !went(&source, tree)
                };
                let goes = match (fill, tree.keep_in) {
                    (Fill::Freshen, None) => own_goes(),
                    // So does one an earlier pass made in a folder of yours ([`Tree::ours`]): it's the copy's own too.
                    (Fill::Freshen, Some(_)) if tree.ours.binary_search(&inside(&copy)).is_ok() => own_goes(),
                    // In a folder of yours, one where the folder has a file or a folder, or a link leading somewhere
                    // else (the plugin pointed it elsewhere since), is named ([`Copied::links_left`]): which is the one
                    // to keep can't be told. Not one whose link in the folder went just now (deleted to be made
                    // again, say): nothing is said of what goes, as for a file.
                    (Fill::Freshen, Some(_)) => {
                        if (!kind.is_symlink() || !same_lead(&source, &copy, &roots)) && !went(&source, tree) {
                            copied.links_left.push(inside(&copy));
                        }
                        false
                    }
                    // Counting: nothing is copied over it. (A new copy has nothing in it.)
                    (Fill::Count | Fill::New, _) => false,
                };
                if !goes {
                    continue;
                }
                remove_link(&copy).map_err(|e| failed(&copy, &e))?;
                tree.has_changed();
                there = None;
            }
            // A file link the pass before copied as its file ([`Tree::as_files`]).
            let as_file = kind.is_symlink() && there.is_some() && tree.as_files.binary_search(&inside(&copy)).is_ok();
            // Any other link where a copy the move made has a real file or folder: the pass before copied a real one
            // there, which turned into a link since. The copy can't take it as it is now, as for a folder above. (In a
            // folder of yours, what's there stays, and is named.)
            if kind.is_symlink() && there.is_some() && !as_file && fill == Fill::Freshen && tree.keep_in.is_none() {
                return Err(changing(&source, "turned into a link"));
            }
            // What a file is copied from: itself, or the file a link leads to, for a file link that's copied as its
            // file.
            let linked;
            let file = if kind.is_symlink() {
                let leads = match (&there, as_file) {
                    (None, _) => linked_file(&source, &kind, &copy, fill, tree, &source_real),
                    (Some(_), true) => as_file_now(&source),
                    // Something at its place already (in a folder of yours, a file or a folder of yours) stays as it
                    // is. Freshening, it's named, as a link of yours is where the folder has a file or a folder (the
                    // plugin, or you, put a link in its place since): which is the one to keep can't be told. Not
                    // when the folder's link went just now (deleted to be made again, say): the other drive has
                    // nothing there then, and nothing is said of what goes, as for a file.
                    (Some(_), false) => {
                        if fill == Fill::Freshen && !went(&source, tree) {
                            copied.links_left.push(inside(&copy));
                        }
                        Ok(None)
                    }
                };
                match leads {
                    Ok(Some(real)) => {
                        if fill != Fill::Count {
                            copied.as_files.push(inside(&copy));
                        }
                        linked = real;
                        linked.as_path()
                    }
                    Ok(None) => {
                        // Made again as a link (or left out: a file link that leads nowhere, which Windows won't
                        // make again). In a folder of yours, only while its folder still leads where it did once the
                        // link was read, as for files waiting to go in: one read through a link that took the folder's
                        // place goes again. One that can't is named, as it's in a folder of yours now.
                        if there.is_none() && fill != Fill::Count {
                            if tree.keep_in.is_some()
                                && let Err(e) = still_real(&source_dir)
                            {
                                return Err(match remove_link(&copy) {
                                    Ok(()) => e,
                                    // Nothing was made there (left out), or something else is there now, the
                                    // plugin's (or yours): it stays.
                                    Err(why) if why.kind() == io::ErrorKind::NotFound => e,
                                    Err(_)
                                        if fs::symlink_metadata(&copy).is_ok_and(|m| !m.file_type().is_symlink()) =>
                                    {
                                        e
                                    }
                                    Err(why) => {
                                        tree.has_changed();
                                        CantMove::Failed(format!(
                                            "{} The link made at {} as it was read couldn't be taken away again \
                                             ({why}): it isn't the plugin's, so delete it.",
                                            part_way(&e),
                                            text(&copy)
                                        ))
                                    }
                                });
                            }
                            if is_link(&copy) {
                                tree.has_changed();
                                // The copy's own, for the next pass ([`Tree::ours`]).
                                if tree.keep_in.is_some() && fill == Fill::Freshen {
                                    copied.written.push(inside(&copy));
                                }
                            }
                        }
                        continue;
                    }
                    Err(_) if went(&source, tree) => continue,
                    Err(e) => return Err(e),
                }
            } else {
                source.as_path()
            };
            if kind.is_dir() {
                match &there {
                    // A folder takes a piece of the drive too (on exFAT), as each file does.
                    None if fill == Fill::Count => copied.bytes = copied.bytes.saturating_add(tree.piece),
                    None => {
                        fs::create_dir(&copy).map_err(|e| failed(&copy, &e))?;
                        tree.has_changed();
                    }
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
            let listed = match if kind.is_symlink() { fs::metadata(file) } else { entry.metadata() } {
                Ok(meta) => meta,
                Err(_) if went(file, tree) => continue,
                Err(e) => return Err(failed(file, &e)),
            };
            // Nor is a pipe or a socket a link leads to, where the link is copied as what it leads to.
            if kind.is_symlink() && special(&listed.file_type()) {
                continue;
            }
            // Never downloaded to be copied (one that went online-only after the check): it stops before the swap. A
            // file a link leads to is copied only where the link can't be made again: counting asks whether it can
            // be, so a copy into a folder of yours stops before anything there changes.
            if is_online_only(&listed)
                && (!kind.is_symlink()
                    || fill != Fill::Count
                    || !*file_links.get_or_insert_with(|| tree.links && makes_file_links()))
            {
                return Err(if kind.is_symlink() {
                    // Where the link leads, as a plain path.
                    let file = real_path(&text(&source)).unwrap_or_else(|| text(file));
                    CantMove::LinkOnlineOnly { link: text(&source), file }
                } else {
                    CantMove::OnlineOnly
                });
            }
            if let Some(m) = &there {
                if m.is_dir() {
                    return Err(CantMove::Failed(format!(
                        "{} is a folder in the copy, so the file of that name couldn't go there.",
                        copy.to_string_lossy()
                    )));
                }
                if same_file(m, &listed) {
                    continue;
                }
            }
            if fill == Fill::Count {
                copied.bytes = copied.bytes.saturating_add(rounded_up(listed.len(), tree.piece));
                continue;
            }
            // About to be copied: as it is now ([`as_now`]). The file a link leads to was looked at just now.
            let meta = if kind.is_symlink() {
                listed
            } else {
                let Some(meta) = as_now(file, listed, tree)? else { continue };
                if is_online_only(&meta) {
                    return Err(CantMove::OnlineOnly);
                }
                meta
            };
            if there.as_ref().is_some_and(|m| same_file(m, &meta)) {
                continue;
            }
            let put = match (fill, tree.keep_in) {
                (Fill::Freshen, Some(_)) => Put::Through,
                (Fill::Freshen, None) => Put::Temporary,
                _ => Put::Straight,
            };
            // Yours, and newer than the one coming in (or any, with `keep_all`): kept aside as the new one goes in, never
            // replaced, so one that never does leaves yours where it was. Only a folder of yours has a `keep_in`, and
            // every file going into one waits to go in ([`Put::Through`]).
            let keep = match (&there, tree.keep_in) {
                (Some(m), Some(keep_in)) => {
                    let rest = inside(&copy);
                    ((tree.keep_all || newer(m, &meta)) && tree.ours.binary_search(&rest).is_err())
                        .then(|| (keep_in.join(&rest), rest))
                }
                _ => None,
            };
            let before = copied.bytes;
            let pieces = Pieces { before, progress, stop };
            match copy_file(file, &copy, &meta, put, &pieces) {
                Err(e) if !matches!(e, CantMove::Stopped) && went(file, tree) => {
                    if fill == Fill::New {
                        let _ = fs::remove_file(&copy);
                    }
                    continue;
                }
                Err(e) => return Err(e),
                Ok(part) => {
                    if tree.keep_in.is_some() && fill == Fill::Freshen {
                        copied.written.push(inside(&copy));
                    }
                    match part {
                        Some(part) => waiting.add(Wait { part, copy, len: meta.len(), keep }),
                        None => {
                            debug_assert!(keep.is_none(), "a file of yours is kept as the new one goes in");
                            tree.has_changed();
                            note_written(&mut copied, &mut recent, copy, meta.len());
                        }
                    }
                }
            }
            copied.bytes = copied.bytes.saturating_add(meta.len());
            progress(copied.bytes);
            if waiting.full() {
                still_real(&source_dir)?;
                waiting.put_in(&mut copied, &mut recent, tree, stop)?;
            }
        }
        // Nor was anything read through one: it, and every folder above it, is still a real folder once all of it was
        // read. Its files went into a copy the move made as they were read (a copy that stops goes); those going into a
        // folder of yours wait to go in, and go once they've been looked at this way.
        still_real(&source_dir)?;
    }
    waiting.put_in(&mut copied, &mut recent, tree, stop)?;
    copied.recent = recent.into_iter().map(|(_, file)| file).collect();
    copied.as_files.sort();
    Ok(copied)
}

/// Whether a folder the copy reads (`dir`: `tree.from`, or a folder in it) leads somewhere else now than as the pass
/// began, so what's read from it may not be the plugin's: why the copy stops, if it does. While nothing changes, each
/// folder is the same place in where `tree.from` really was then (`source_real`), and a folder: one look at where it
/// really is and what it is. When it isn't, `tree.from` is named if it changed (it turned into a link or a file, or, a
/// link itself, or reached through one, leads somewhere else now: moving back, the copy on the other drive can be a
/// link of yours); otherwise the topmost folder in it that turned into a link or a file, looked for name by name from
/// the top, never through a link. `None` while it's the same folder, renamed in other capitals or not
/// ([`still_same_place`]), and when it's gone: what goes is passed over, while `tree.from` is there ([`went`]). One that
/// can't be looked at (not gone: the system says no) stops it, as whether it changed can't be told, saying what the
/// system said. When `tree.from` leads nowhere now, a real folder as the pass began (`from_was_link` says) that's a
/// link now changed (pointed at nothing, after something was read through it). Otherwise its drive went (unplugged),
/// or the link it is leads nowhere now, and where what was read came from can't be told: into a folder of yours
/// nothing goes then, and the copy stops. A copy the move made stops when what's still to be read can't be, and
/// moving back looks first that the copy is where it was ([`not_the_link_now`]).
fn led_elsewhere(dir: &Path, tree: &Tree, source_real: &Path, from_was_link: bool) -> Option<CantMove> {
    const ELSEWHERE: &str = "turned into a link or a file, or led somewhere else";
    let rest = dir.strip_prefix(tree.from).ok()?;
    let now = fs::canonicalize(dir);
    if now.as_ref().is_ok_and(|now| still_same_place(now, &source_real.join(rest)) && now.is_dir()) {
        return None;
    }
    match fs::canonicalize(tree.from) {
        Ok(now) if still_same_place(&now, source_real) && now.is_dir() => {}
        Ok(_) => return Some(changing(tree.from, ELSEWHERE)),
        Err(_) if !from_was_link && is_link(tree.from) => return Some(changing(tree.from, "turned into a link")),
        Err(e) => return tree.keep_in.is_some().then(|| failed(tree.from, &e)),
    }
    // The topmost one that isn't a real folder now, if it's there: it turned into a link or a file.
    let turned = not_real_on_the_way(tree.from, rest.components()).filter(|at| fs::symlink_metadata(at).is_ok());
    match (turned, now) {
        (Some(at), _) => Some(changing(&at, "turned into a link or a file")),
        (None, _) if not_there(dir) => None,
        // It can't be looked at (the system says no, or the drive doesn't answer): what it says is why it stops.
        (None, Err(e)) if e.kind() != io::ErrorKind::NotFound => Some(failed(dir, &e)),
        // It changed just now, or in a way the app can't tell: it stops all the same.
        (None, _) => Some(changing(dir, ELSEWHERE)),
    }
}

/// Whether two places where things really are ([`fs::canonicalize`]) are the same: the very same, or but for capitals
/// where the drive doesn't tell them apart (a folder renamed in other capitals meanwhile). That's when `then` still
/// leads to `now`, with no link on the way: `then` had none, being where something really was, so one there now (put
/// in the place of a folder renamed in other capitals, where the drive tells them apart) leads to another folder. A
/// drive mounted in a folder, which Windows takes for a link, is where it really is itself: it's no link put there.
fn still_same_place(now: &Path, then: &Path) -> bool {
    // Without a separator at the end (`then` for the folder itself is where it was joined to nothing): through one,
    // the system follows a link, so a link put in the folder's place would pass for a folder.
    let then: PathBuf = then.components().collect();
    let then = then.as_path();
    now == then
        || key(&text(now)) == key(&text(then))
            && then
                .ancestors()
                .all(|at| at.file_name().is_none() || !is_link(at) || fs::canonicalize(at).is_ok_and(|real| real == at))
            && fs::canonicalize(then).is_ok_and(|real| real == now)
}

/// The file a link the pass before copied as its file ([`Tree::as_files`]) leads to now, to bring its copy up to date.
/// One that leads to no file now (a folder, or nowhere) stops the copy: it can't take the link as it is now.
fn as_file_now(link: &Path) -> Result<Option<PathBuf>, CantMove> {
    match fs::canonicalize(link) {
        Ok(real) if real.is_file() => Ok(Some(real)),
        _ => Err(changing(link, "changed")),
    }
}

/// A link inside the folder being copied, with nothing at its place in the copy: made again there as a link
/// ([`copy_link`]). `Some` is the file it leads to, to be copied as a file instead: the file link couldn't be made
/// again, or, counting, may not be (Windows doesn't make one without Developer Mode, and a drive that holds no links
/// can't take one), so its file is counted. Counting for a drive that holds no links ([`Tree::links`]), any other link
/// that would be made again as a link stops the count ([`CantMove::NoLinksThere`]), but for one to a pipe or a socket
/// ([`special`]): the copy passes that over where it can't make it again, as it would the file it leads to.
fn linked_file(
    source: &Path,
    kind: &fs::FileType,
    copy: &Path,
    fill: Fill,
    tree: &Tree,
    source_real: &Path,
) -> Result<Option<PathBuf>, CantMove> {
    if fill == Fill::Count {
        let leads_to = fs::metadata(source).ok();
        if leads_to.as_ref().is_some_and(fs::Metadata::is_file) {
            return Ok((cfg!(windows) || !tree.links).then(|| source.to_path_buf()));
        }
        let to_special = leads_to.as_ref().is_some_and(|m| special(&m.file_type()));
        if !tree.links && !to_special && remade_as_link(kind, leads_to.as_ref()) {
            return Err(CantMove::NoLinksThere(text(source)));
        }
        return Ok(None);
    }
    copy_link(source, kind, copy, tree, source_real)
}

/// Whether a link that doesn't lead to a file (`leads_to`: what it leads to now, if anything) is made again as a link
/// ([`copy_link`]): one to a folder always is, and one that leads nowhere now is too, but on Windows only a folder link
/// (a file link there that Windows won't make again is left out).
fn remade_as_link(kind: &fs::FileType, leads_to: Option<&fs::Metadata>) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        leads_to.map_or_else(|| kind.is_symlink_dir(), fs::Metadata::is_dir)
    }
    #[cfg(not(windows))]
    {
        let _ = (kind, leads_to);
        true
    }
}

/// Whether Windows makes file links for the app now: that takes Developer Mode (or administrator rights). Tried by
/// making one in the system's temporary folder and removing it again, never in a folder of yours. (The drive it's
/// copied to must hold links too: [`Tree::links`].) Elsewhere it always does.
fn makes_file_links() -> bool {
    #[cfg(windows)]
    {
        use std::sync::atomic::AtomicU64;
        static TRIES: AtomicU64 = AtomicU64::new(0);
        let n = TRIES.fetch_add(1, Ordering::Relaxed);
        let temp = std::env::temp_dir();
        let probe = temp.join(format!("Music Organiser link check {} {n}", std::process::id()));
        let made = std::os::windows::fs::symlink_file(temp.join("Music Organiser link check"), &probe).is_ok();
        if made {
            let _ = fs::remove_file(&probe);
        }
        made
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// A file about to be copied, as it is now; `listed` is how its folder's listing found it. On Windows that's how it was
/// when the folder was listed (or, for a file something has open, when it was last closed), and a big folder takes a
/// long while to copy, so it's looked at again: a file that changed meanwhile would seem not to copy whole. (Elsewhere
/// the listing looks at each file as it's asked.) `None` when it's gone: it's passed over, as a plugin's lock file is.
/// One that's a folder or a link now is passed over too, for the next pass to copy as what it is, but in the last pass
/// ([`Tree::last`]) it stops the copy: it would be missing from the copy.
///
/// Only a file about to be copied is looked at again, when it's opened to be copied anyway: looking at every name in
/// every pass would open each file once more on Windows, which on a spinning drive is a wait for each of them.
fn as_now(file: &Path, listed: fs::Metadata, tree: &Tree) -> Result<Option<fs::Metadata>, CantMove> {
    let now = if cfg!(windows) {
        match fs::symlink_metadata(file) {
            Ok(meta) => meta,
            Err(_) if went(file, tree) => return Ok(None),
            Err(e) => return Err(failed(file, &e)),
        }
    } else {
        listed
    };
    match (now.is_file(), tree.last) {
        (true, _) => Ok(Some(now)),
        (false, false) => Ok(None),
        (false, true) => Err(changing(file, "turned into a folder or a link")),
    }
}

/// A folder's listing, whole, looking at Stop between names. A folder that goes while it's read gives what was read
/// ([`went`]).
fn listed_whole(
    listing: fs::ReadDir,
    folder: &Path,
    tree: &Tree,
    stop: &AtomicBool,
) -> Result<Vec<fs::DirEntry>, CantMove> {
    let mut whole = Vec::new();
    for entry in listing {
        if stop.load(Ordering::Relaxed) {
            return Err(CantMove::Stopped);
        }
        match entry {
            Ok(entry) => whole.push(entry),
            Err(_) if went(folder, tree) => break,
            Err(e) => return Err(failed(folder, &e)),
        }
    }
    Ok(whole)
}

/// The file a temporary file ([`part_of`]) was written for, by name, when `name` is one: what a copy writes over an
/// older file until it's whole, which a copy cut off part way leaves. The app's own, never a plugin's.
pub(crate) fn own_name(name: &str) -> Option<&str> {
    name.strip_suffix(PART_SUFFIX).filter(|own| !own.is_empty())
}

/// The names in a folder's listing that differ only in capitals from another in it, as [`key`] gives them.
fn twins(entries: &[fs::DirEntry]) -> HashSet<String> {
    let mut seen = HashSet::with_capacity(entries.len());
    let mut twins = HashSet::new();
    for entry in entries {
        let name = key(&entry.file_name().to_string_lossy());
        if seen.contains(&name) {
            twins.insert(name);
        } else {
            seen.insert(name);
        }
    }
    twins
}

/// Whether a folder lists `name` by that very name, capitals and all. Its names are read the first time (into
/// `names`), and only then.
fn named_in(folder: &Path, names: &mut Option<HashSet<OsString>>, name: &OsStr) -> bool {
    names
        .get_or_insert_with(|| fs::read_dir(folder).into_iter().flatten().flatten().map(|e| e.file_name()).collect())
        .contains(name)
}

/// A file copied into a folder of yours under its temporary name, waiting to go in ([`Waiting`]).
struct Wait {
    part: PathBuf,
    /// Where it goes.
    copy: PathBuf,
    len: u64,
    /// Where your file there is kept first, as it goes in, and its path inside the copy ([`Copied::kept`]).
    keep: Option<(PathBuf, PathBuf)>,
}

impl Wait {
    /// Puts it in place, keeping yours there first when it's to be kept (when it's still there). If it can't go in,
    /// yours goes back where it was.
    fn go_in(&self, copied: &mut Copied) -> Result<(), CantMove> {
        let keep = self.keep.as_ref().filter(|_| !not_there(&self.copy));
        if let Some((kept, rest)) = keep {
            keep_aside(&self.copy, kept, rest)?;
        }
        if let Err(e) = put_in_place(&self.part, &self.copy) {
            if let Some((kept, rest)) = keep {
                put_back(kept, rest, &self.copy);
            }
            return Err(e);
        }
        if let Some((_, rest)) = keep {
            copied.kept.push(rest.clone());
        }
        Ok(())
    }
}

/// Files copied into a folder of yours under their temporary names, waiting to be written through to the disk
/// together and put in place ([`Put::Through`]), each keeping yours first when it's to be kept. Any still waiting when
/// it goes (the copy stopped, or went wrong) are removed: what they'd have replaced stays where it was.
#[derive(Default)]
struct Waiting {
    files: Vec<Wait>,
    bytes: u64,
    /// Each waiting file's temporary name and place, as [`key`] gives them.
    names: HashSet<String>,
}

impl Waiting {
    fn add(&mut self, wait: Wait) {
        self.bytes = self.bytes.saturating_add(wait.len);
        self.names.insert(key(&text(&wait.part)));
        self.names.insert(key(&text(&wait.copy)));
        self.files.push(wait);
    }

    /// Whether as many are waiting as should ([`WAITING_FILES`], [`WAITING_BYTES`]).
    fn full(&self) -> bool {
        self.files.len() >= WAITING_FILES || self.bytes >= WAITING_BYTES
    }

    /// Whether a file going to `copy` would meet one that's waiting: its place, or its temporary name, is one of
    /// theirs (two names that differ only in capitals are one on a drive that doesn't tell them apart). It goes once
    /// those are in, as it would have before.
    fn meets(&self, copy: &Path) -> bool {
        !self.files.is_empty()
            && (self.names.contains(&key(&text(copy))) || self.names.contains(&key(&text(&part_of(copy)))))
    }

    /// Writes the waiting files through to the disk, spread over the cores of the pool it's called in, then puts each
    /// in place, noting the change in `tree` ([`Tree::changed`]). Stop is looked at before each: stopped, none goes in.
    /// One that couldn't be written through now is noted for the copy's last write-through ([`write_through`]), as a
    /// file put in elsewhere is.
    fn put_in(
        &mut self,
        copied: &mut Copied,
        recent: &mut VecDeque<(Instant, PathBuf)>,
        tree: &Tree,
        stop: &AtomicBool,
    ) -> Result<(), CantMove> {
        if self.files.is_empty() {
            return Ok(());
        }
        let flushed: Vec<bool> =
            self.files.par_iter().map(|wait| !stop.load(Ordering::Relaxed) && flush_file(&wait.part)).collect();
        if stop.load(Ordering::Relaxed) {
            return Err(CantMove::Stopped);
        }
        let files = std::mem::take(&mut self.files);
        self.bytes = 0;
        self.names.clear();
        for (n, (wait, flushed)) in files.iter().zip(flushed).enumerate() {
            if let Err(e) = wait.go_in(copied) {
                files.iter().skip(n).for_each(|wait| remove_part(&wait.part));
                return Err(e);
            }
            tree.has_changed();
            if !flushed {
                note_written(copied, recent, wait.copy.clone(), wait.len);
            }
        }
        Ok(())
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        for wait in &self.files {
            remove_part(&wait.part);
        }
    }
}

/// Notes a file a copy wrote, to be written through to the disk before the copy goes into use ([`write_through`]): a
/// big one ([`IN_PIECES_FROM`] or more) whenever it was written, a small one while it's among the last written.
fn note_written(copied: &mut Copied, recent: &mut VecDeque<(Instant, PathBuf)>, file: PathBuf, len: u64) {
    if len >= IN_PIECES_FROM {
        copied.big.push(file);
    } else {
        wrote(recent, file);
    }
}

/// Notes a small file a copy just wrote, keeping only those written in the last [`WRITTEN_THROUGH`], at most
/// [`THROUGH_MOST`] of them.
fn wrote(recent: &mut VecDeque<(Instant, PathBuf)>, file: PathBuf) {
    let now = Instant::now();
    recent.push_back((now, file));
    while recent.len() > THROUGH_MOST || recent.front().is_some_and(|(at, _)| now.duration_since(*at) > WRITTEN_THROUGH)
    {
        recent.pop_front();
    }
}

/// A file's size as a whole number of a drive's pieces (as it is, when they aren't known).
fn rounded_up(len: u64, piece: u64) -> u64 {
    if piece == 0 { len } else { len.div_ceil(piece).saturating_mul(piece) }
}

/// Whether something a copy reads went while it was read (a plugin's lock file deleted, or its cache cleared), so it's
/// passed over: nothing is there now, and the folder being copied still is. Anything else stops the copy: a drive that
/// drops out, or a folder that can't be read, isn't taken for files deleted (they'd be missing from the copy, while
/// still where they were).
fn went(path: &Path, tree: &Tree) -> bool {
    // Where it really is: a folder that's a link itself is there only while what it leads to is.
    not_there(path) && fs::metadata(tree.from).is_ok()
}

/// What a copy says when something in the folder changed in a way it can't copy as it is now (`how` says how), as the
/// copy went: a plugin or the DAW is at work in it.
fn changing(path: &Path, how: &str) -> CantMove {
    CantMove::Failed(format!(
        "{} {how} while it was being copied: something is changing the folder. Close your DAW and the plugin, then \
         try again.",
        text(path)
    ))
}

/// Whether a name in a folder is neither a file, a folder nor a link: a pipe or a socket (or a device) on a Mac or
/// Linux. A copy passes it over: it holds nothing to copy, and reading a pipe would wait for ever for something to
/// write into it. (Windows lists its own socket files, which are rare, as files: one is copied as any file is, and the
/// copy stops if it can't be.)
fn special(kind: &fs::FileType) -> bool {
    !kind.is_file() && !kind.is_dir() && !kind.is_symlink()
}

/// What a copy that went wrong says.
pub(crate) fn failed(path: &Path, e: &io::Error) -> CantMove {
    CantMove::Failed(format!("Copying stopped at {} ({e}).", path.to_string_lossy()))
}

/// Moves a file of yours out of the way, to the same place inside the folder kept beside the plugin's: `keep`, which
/// is `rest` (its path inside the plugin's folder) inside it. One that can't be moved stays where it was.
fn keep_aside(file: &Path, keep: &Path, rest: &Path) -> Result<(), CantMove> {
    if fs::symlink_metadata(keep).is_ok() {
        return Err(CantMove::Failed(format!("{} is there already.", keep.to_string_lossy())));
    }
    if let Some(parent) = keep.parent() {
        fs::create_dir_all(parent).map_err(|e| failed(parent, &e))?;
    }
    fs::rename(file, keep).map_err(|e| {
        clear_keeping(keep, rest);
        failed(file, &e)
    })
}

/// Puts a file of yours kept aside ([`keep_aside`]) back where it was, when nothing has taken its place, trying again
/// for a few seconds while something (a virus checker) looks in. One that can't go back stays kept.
fn put_back(kept: &Path, rest: &Path, file: &Path) {
    if not_there(file) && rename_back(kept, file).is_ok() {
        clear_keeping(kept, rest);
    }
}

/// Removes the folders made to keep a file at `kept` (`rest` inside the folder of kept files, that folder too), those
/// left empty: once it's gone back, or couldn't be moved there.
fn clear_keeping(kept: &Path, rest: &Path) {
    let mut folder = kept.parent();
    for _ in rest.components() {
        match folder {
            Some(dir) if fs::remove_dir(dir).is_ok() => folder = dir.parent(),
            _ => break,
        }
    }
}

/// Files at least this big are copied a piece at a time, so Stop and the progress bar don't wait for the whole file
/// (Omnisphere's sound sources run to tens of GB). Smaller ones are copied by the system in one go, which on a Mac also
/// keeps a file's extended attributes (Finder tags, say); a big file's pieces don't.
const IN_PIECES_FROM: u64 = 64 << 20;

/// How much of a big file is copied between looks at Stop.
const PIECE: usize = 8 << 20;

/// For a big file: the bytes copied before it, where to say how far the copy has got, and Stop.
struct Pieces<'a> {
    before: u64,
    progress: &'a (dyn Fn(u64) + Sync),
    stop: &'a AtomicBool,
}

/// How [`copy_file`] puts a file in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Put {
    /// Straight in: a copy just made has nothing there.
    Straight,
    /// Under a temporary name first, taking its place only once whole, so one that fails leaves what was there as it
    /// was.
    Temporary,
    /// The same, and written through to the disk before it takes its place: into a folder of yours (the old copy, or
    /// the copy on the other drive), where a power cut mustn't leave a file of the right size and time whose bytes
    /// never reached the disk (the next try would take it for its twin and leave it). It waits under its temporary
    /// name, to be written through with others and put in place ([`Waiting`]).
    Through,
}

/// Copies one file, keeps its time and checks it arrived whole, put in as `put` says. `Some` is the temporary copy
/// left waiting to be written through and put in place ([`Put::Through`]).
fn copy_file(
    source: &Path,
    copy: &Path,
    meta: &fs::Metadata,
    put: Put,
    pieces: &Pieces,
) -> Result<Option<PathBuf>, CantMove> {
    let when = meta.modified().map_err(|e| failed(source, &e))?;
    if put == Put::Straight {
        return copy_bytes(source, copy, meta, pieces)
            .and_then(|()| keep_time(copy, when).map_err(|e| failed(source, &e)))
            .and_then(|()| whole(copy, meta, source))
            .map(|()| None);
    }
    let part = part_of(copy);
    match fs::symlink_metadata(&part) {
        Ok(m) if !m.is_file() => {
            return Err(CantMove::Failed(format!("{} is in the way.", part.to_string_lossy())));
        }
        // Left by a copy that stopped.
        Ok(_) => remove_part(&part),
        Err(_) => {}
    }
    let written = copy_bytes(source, &part, meta, pieces)
        .and_then(|()| keep_time(&part, when).map_err(|e| failed(source, &e)))
        .and_then(|()| whole(&part, meta, source));
    match written {
        Ok(()) if put == Put::Through => Ok(Some(part)),
        Ok(()) => put_in_place(&part, copy).map(|()| None).inspect_err(|_| remove_part(&part)),
        Err(e) => {
            remove_part(&part);
            Err(e)
        }
    }
}

/// What a file going in over an older one is called until it's whole ([`PART_SUFFIX`]).
fn part_of(copy: &Path) -> PathBuf {
    let mut name = copy.file_name().unwrap_or_default().to_os_string();
    name.push(PART_SUFFIX);
    copy.with_file_name(name)
}

/// Copies a file's bytes to a new file. A big one goes a piece at a time, looking at Stop and saying how far it's got
/// between pieces, and then gets the file's permissions as the system's copy gives them (on Windows, its marks:
/// read-only, hidden and so on). It's written through to the disk later: with the rest of the copy before the copy goes
/// into use ([`write_through`]), or, into a folder of yours, before it takes its place ([`Put::Through`]). A stop
/// leaves the part copied, which goes with the rest of the copy (or as a temporary copy does).
fn copy_bytes(source: &Path, to: &Path, meta: &fs::Metadata, pieces: &Pieces) -> Result<(), CantMove> {
    if meta.len() < IN_PIECES_FROM {
        return fs::copy(source, to).map(drop).map_err(|e| failed(source, &e));
    }
    let mut from = fs::File::open(source).map_err(|e| failed(source, &e))?;
    let mut into = fs::File::options().write(true).create_new(true).open(to).map_err(|e| failed(to, &e))?;
    let mut buffer = vec![0; PIECE];
    let mut done: u64 = 0;
    loop {
        if pieces.stop.load(Ordering::Relaxed) {
            return Err(CantMove::Stopped);
        }
        let read = match from.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(failed(source, &e)),
        };
        into.write_all(buffer.get(..read).unwrap_or_default()).map_err(|e| failed(to, &e))?;
        done = done.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        (pieces.progress)(pieces.before.saturating_add(done));
    }
    drop(into);
    fs::set_permissions(to, meta.permissions()).map_err(|e| failed(to, &e))
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
pub(crate) fn set_read_only(path: &Path, read_only: bool) -> bool {
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

/// Whether two files look the same: the same size, and times no further apart than the coarser drive's clock steps.
pub(crate) fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.len() == b.len()
        && match (a.modified(), b.modified()) {
            (Ok(x), Ok(y)) => x.duration_since(y).unwrap_or_else(|e| e.duration()) < steps(x, y),
            _ => false,
        }
}

/// Whether `a` was changed later than `b`, by more than the coarser drive's clock steps.
pub(crate) fn newer(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    matches!((a.modified(), b.modified()), (Ok(x), Ok(y)) if x.duration_since(y).is_ok_and(|d| d >= steps(x, y)))
}

/// How far apart two times of one file can be, by the coarser of the two: a time kept to the second (no fraction:
/// FAT32, or another drive keeping seconds) may be up to two seconds off; one kept to a hundredth (exFAT), a
/// hundredth; finer ones (NTFS, a Mac's and Linux's drives) are copied as they are, so they must match. A file written
/// twice within two seconds, the same size, isn't taken for the copy made between the writes, as it was when every
/// drive was given two seconds. (A fine drive's time that happens to fall on a whole second, about one in ten million,
/// only gets the wider window.)
fn steps(a: SystemTime, b: SystemTime) -> Duration {
    let step = |t: SystemTime| match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) if d.subsec_nanos() == 0 => SAME_TIME,
        Ok(d) if d.subsec_nanos() % 10_000_000 == 0 => SAME_TIME_EXFAT,
        Ok(_) => SAME_TIME_FINE,
        // Before 1970: can't tell, so the widest.
        Err(_) => SAME_TIME,
    };
    step(a).max(step(b))
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
        Ok(rest) => tree.links_lead.unwrap_or(tree.settled).join(rest),
        Err(_) => place,
    }
}

/// A path as Windows writes it for people: without the `\\?\` a link's own wording (or a real path) can start with.
fn plainly(path: &Path) -> PathBuf {
    let shown = path.to_string_lossy();
    // Only a drive letter's (`\\?\C:\`) or a network share's: one naming a drive by its id (`\\?\Volume{…}\`, a drive
    // mounted in a folder) means nothing without it.
    let lettered = |rest: &str| rest.as_bytes().get(1) == Some(&b':') && rest.as_bytes()[0].is_ascii_alphabetic();
    match (shown.strip_prefix(r"\\?\UNC\"), shown.strip_prefix(r"\\?\")) {
        (Some(network), _) => PathBuf::from(format!(r"\\{network}")),
        (None, Some(local)) if lettered(local) => PathBuf::from(local),
        _ => path.to_path_buf(),
    }
}

/// Makes a link in the copy leading where the original leads ([`lead`]). One that leads nowhere now (a drive that
/// isn't plugged in) is made again too. A file link Windows won't let be made again is to be copied as the file it
/// leads to: `Some` is that file, copied as any file is.
fn copy_link(
    link_path: &Path,
    kind: &fs::FileType,
    copy: &Path,
    tree: &Tree,
    source_real: &Path,
) -> Result<Option<PathBuf>, CantMove> {
    let couldnt = |e: io::Error| {
        CantMove::Failed(format!("{} is a link that couldn't be made again ({e}).", link_path.to_string_lossy()))
    };
    // One that doesn't take leaves nothing behind (Windows makes a junction from an empty folder, which would stay).
    let didnt_take = |e: io::Error| {
        clear_failed_link(copy);
        couldnt(e)
    };
    let to = lead_in_copy(worded_place(link_path, &fs::read_link(link_path).map_err(couldnt)?), tree, source_real);
    let Ok(real) = fs::canonicalize(link_path) else {
        // It leads nowhere now (a drive that isn't plugged in): made again as it's worded.
        return copy_dangling(kind, &to, copy).map(|()| None).map_err(didnt_take);
    };
    if real.is_dir() {
        return link(copy, &to).map(|()| None).map_err(didnt_take);
    }
    Ok(file_link(&to, copy).is_err().then_some(real))
}

/// Where a link leads by its wording (`words`, as [`fs::read_link`] gives it), worked out from the folder it's in, not
/// where it ends up: a link into another moved folder (C:\B, on D: now) keeps leading to C:\B, which that folder's
/// own link keeps reaching, moved back or moved again. One worded from where it sits ("..\Shared") is worked out as
/// the system does: from where its folder really is.
fn worded_place(link: &Path, words: &Path) -> PathBuf {
    let base = || link.parent().map(|p| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    plainly(&tidy(&if words.is_absolute() {
        words.to_path_buf()
    } else {
        base().map_or_else(|| words.to_path_buf(), |p| p.join(words))
    }))
}

/// Where a link in the folder being copied leads in the copy, from where it leads by its wording ([`worded_place`]):
/// the same place, but for a place inside the folder ([`lead`]), as [`copy_link`] makes it.
fn lead_in_copy(place: PathBuf, tree: &Tree, source_real: &Path) -> PathBuf {
    let from = plainly(&tidy(tree.from));
    if place.starts_with(&from) { lead(place, &from, tree) } else { lead(place, &plainly(source_real), tree) }
}

/// Whether the link at `copy`, which the pass before made again from the folder's link at `source`, is still worded
/// as that one would be made now ([`copy_link`]): one the plugin pointed elsewhere since isn't.
fn made_as_worded(source: &Path, copy: &Path, tree: &Tree, source_real: &Path) -> bool {
    match (fs::read_link(source), fs::read_link(copy)) {
        (Ok(words), Ok(made)) => {
            plainly(&made) == plainly(&lead_in_copy(worded_place(source, &words), tree, source_real))
        }
        _ => false,
    }
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
    use std::collections::VecDeque;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{
        CantMove, Copied, Drives, SystemDrives, Tree, Wait, Waiting, in_itself, rest_after, same_lead,
        still_same_place, write_through,
    };

    /// A folder renamed in other capitals is the same place only where the drive doesn't tell capitals apart (Windows
    /// and a Mac): where it does, a folder named the same but for capitals is another folder.
    #[test]
    fn a_folder_in_other_capitals_is_the_same_place_only_where_the_drive_doesnt_tell_them_apart() {
        let dir = std::env::temp_dir().join("mo-space-unit-same-place");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Caps")).unwrap();
        fs::create_dir(dir.join("Other")).unwrap();
        let then = fs::canonicalize(dir.join("Caps")).unwrap();
        assert!(still_same_place(&then, &then));
        assert!(!still_same_place(&fs::canonicalize(dir.join("Other")).unwrap(), &then));
        if dir.join("caps").exists() {
            // Looked up by the name it had, as the copy does: where it really is now is in its new capitals (on
            // Windows), and the old name still leads there.
            fs::rename(dir.join("Caps"), dir.join("caps")).unwrap();
            assert!(still_same_place(&fs::canonicalize(dir.join("Caps")).unwrap(), &then));
        } else {
            fs::create_dir(dir.join("caps")).unwrap();
            assert!(!still_same_place(&fs::canonicalize(dir.join("caps")).unwrap(), &then));
            // Nor is a link put in the place of the folder renamed in other capitals, under its old name: it leads
            // there, in other capitals, but through a link.
            #[cfg(unix)]
            {
                fs::rename(dir.join("Caps"), dir.join("CAPS")).unwrap();
                std::os::unix::fs::symlink("CAPS", dir.join("Caps")).unwrap();
                assert!(!still_same_place(&fs::canonicalize(dir.join("Caps")).unwrap(), &then));
            }
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Two links whose places can't be told (nothing that can be read is there) aren't taken for the same; two that
    /// lead to the same place are, worded with a whole path or from where they sit.
    #[test]
    fn links_are_the_same_only_when_where_they_lead_can_be_told() {
        let dir = std::env::temp_dir().join("mo-space-unit-same-lead");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Target")).unwrap();
        assert!(!same_lead(&dir.join("one"), &dir.join("two"), &[]));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("Target"), dir.join("whole")).unwrap();
            std::os::unix::fs::symlink("Target", dir.join("near")).unwrap();
            assert!(same_lead(&dir.join("whole"), &dir.join("near"), &[]));
            assert!(!same_lead(&dir.join("whole"), &dir.join("one"), &[]));
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A path inside a folder is in that folder itself only by names, every folder on the way a real one: never by
    /// "..", which leads out, nor through a link.
    #[test]
    fn a_path_is_in_a_folder_itself_only_by_names_with_no_link_on_the_way() {
        let dir = std::env::temp_dir().join("mo-space-unit-in-itself");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Folder/Sub")).unwrap();
        let folder = dir.join("Folder");
        assert!(in_itself(&folder, Path::new("x")));
        assert!(in_itself(&folder, Path::new("Sub/x")));
        assert!(!in_itself(&folder, Path::new("../x")));
        assert!(!in_itself(&folder, Path::new("Sub/../../x")));
        assert!(!in_itself(&folder, Path::new("Gone/x")));
        assert!(!in_itself(&folder, Path::new("")));
        assert!(!in_itself(&dir.join("Gone"), Path::new("x")));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("Folder/Sub"), dir.join("Folder/Link")).unwrap();
            assert!(!in_itself(&folder, Path::new("Link/x")));
            std::os::unix::fs::symlink(&folder, dir.join("Through")).unwrap();
            assert!(!in_itself(&dir.join("Through"), Path::new("x")));
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_drive_the_temporary_folder_is_on_holds_links() {
        assert!(SystemDrives.holds_links(&std::env::temp_dir()));
        // Windows names its kind (NTFS, or ReFS on a Dev Drive), from its list of drives: a lookup that found no drive
        // would count every drive as holding links.
        #[cfg(windows)]
        {
            let kind = crate::clean::volume_of(&crate::text(&crate::really(&std::env::temp_dir()))).map(|v| v.kind);
            assert!(kind.as_deref().is_some_and(|k| !k.is_empty() && super::kind_holds_links(k)), "{kind:?}");
            for kind in ["FAT", "FAT32", "exFAT", "UDF", "HFS+", "APFS"] {
                assert!(!super::kind_holds_links(kind), "{kind}");
            }
            for kind in ["NTFS", "ntfs", "ReFS", "Btrfs", ""] {
                assert!(super::kind_holds_links(kind), "{kind}");
            }
        }
    }

    #[test]
    fn writing_a_copy_through_to_the_disk_stops_when_you_press_stop() {
        let dir = std::env::temp_dir().join("mo-space-unit-write-through");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let patch = dir.join("Patch.prt");
        fs::write(&patch, "patch").unwrap();
        // A file both passes wrote, and a big one that's gone since (it's let be).
        let first = Copied { recent: vec![patch.clone()], big: vec![dir.join("gone.db")], ..Copied::default() };
        let again = Copied { recent: vec![patch.clone()], ..Copied::default() };
        assert_eq!(write_through(&[&first, &again], &AtomicBool::new(false)), Ok(()));
        // Stopped, it says so: the copy mustn't go into use.
        assert_eq!(write_through(&[&first, &again], &AtomicBool::new(true)), Err(CantMove::Stopped));
        assert_eq!(fs::read_to_string(&patch).unwrap(), "patch");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn times_match_as_finely_as_the_coarser_drive_keeps_them() {
        use super::steps;
        use std::time::{Duration, SystemTime, UNIX_EPOCH};
        let at = |secs: u64, nanos: u32| UNIX_EPOCH + Duration::new(secs, nanos);
        let fine = at(1_700_000_000, 123_456_700);
        // NTFS to NTFS: a write 1.5 seconds later is another write.
        assert_eq!(steps(fine, fine + Duration::from_millis(1500)), Duration::from_micros(1));
        // exFAT keeps hundredths, FAT32 even seconds.
        assert_eq!(steps(fine, at(1_700_000_000, 120_000_000)), Duration::from_millis(10));
        assert_eq!(steps(fine, at(1_700_000_000, 0)), Duration::from_secs(2));
        assert_eq!(steps(SystemTime::UNIX_EPOCH - Duration::from_secs(1), fine), Duration::from_secs(2));
    }

    #[test]
    fn files_waiting_to_go_into_a_folder_of_yours_go_in_together_or_not_at_all() {
        let dir = std::env::temp_dir().join("mo-space-unit-waiting");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, text: &str| {
            let path = dir.join(name);
            fs::write(&path, text).unwrap();
            path
        };
        let (a, b) = (file("a.prt", "yours"), file("b.prt", "yours"));
        let (mut copied, mut recent) = (Copied::default(), VecDeque::new());
        let wait = |part: PathBuf, copy: &PathBuf| Wait { part, copy: copy.clone(), len: 3, keep: None };
        let (changed, unchanged) = (AtomicBool::new(false), AtomicBool::new(false));
        let tree = Tree { changed: Some(&changed), ..Tree::new(&dir, &dir, &dir) };
        let mut waiting = Waiting::default();
        waiting.add(wait(file("a.prt.mo-copying", "new"), &a));
        waiting.add(wait(file("b.prt.mo-copying", "new"), &b));
        assert_eq!(waiting.put_in(&mut copied, &mut recent, &tree, &AtomicBool::new(false)), Ok(()));
        assert_eq!((fs::read_to_string(&a).unwrap(), fs::read_to_string(&b).unwrap()), ("new".into(), "new".into()));
        assert!(changed.load(Ordering::Relaxed));
        // Written through as they went in, so not again before the copy goes into use.
        assert!(recent.is_empty() && copied.big.is_empty());
        // Stopped, none goes in, and what waited goes: yours stays as it was, and nothing has changed.
        let tree = Tree { changed: Some(&unchanged), ..tree };
        waiting.add(wait(file("a.prt.mo-copying", "newer"), &a));
        assert_eq!(waiting.put_in(&mut copied, &mut recent, &tree, &AtomicBool::new(true)), Err(CantMove::Stopped));
        assert!(!unchanged.load(Ordering::Relaxed));
        drop(waiting);
        assert_eq!(fs::read_to_string(&a).unwrap(), "new");
        assert!(!dir.join("a.prt.mo-copying").exists());
        // One that can't be written through then (here it can't be opened, but by the administrator) still goes in,
        // and is written through again before the copy goes into use.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let part = file("c.prt.mo-copying", "new");
            fs::set_permissions(&part, fs::Permissions::from_mode(0o200)).unwrap();
            let cant_open = fs::File::open(&part).is_err();
            let mut waiting = Waiting::default();
            waiting.add(wait(part, &dir.join("c.prt")));
            assert_eq!(waiting.put_in(&mut copied, &mut recent, &tree, &AtomicBool::new(false)), Ok(()));
            assert!(dir.join("c.prt").exists());
            let noted: Vec<PathBuf> = recent.iter().map(|(_, file)| file.clone()).collect();
            assert_eq!(noted == [dir.join("c.prt")], cant_open, "{noted:?}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_link_copied_as_its_file_is_brought_up_to_date_and_any_other_link_at_a_file_stops_the_copy() {
        use super::{Fill, copy_tree, file_link};
        let dir = std::env::temp_dir().join("mo-space-unit-as-files");
        let _ = fs::remove_dir_all(&dir);
        let (from, to) = (dir.join("From"), dir.join("To"));
        fs::create_dir_all(&from).unwrap();
        fs::create_dir_all(&to).unwrap();
        let shared = dir.join("Shared.db");
        fs::write(&shared, "shared").unwrap();
        // Only where the system lets a file link be made (Windows: with Developer Mode, or as administrator).
        if file_link(&shared, &from.join("Link.db")).is_err() {
            return;
        }
        let stop = AtomicBool::new(false);
        let freshen = |tree: &Tree| copy_tree(tree, Fill::Freshen, &|_| {}, &stop);
        let link = [PathBuf::from("Link.db")];
        // The pass before copied the link as its file (it couldn't make it again), as it was then. The file it leads to
        // has been saved again since: it's copied again.
        fs::write(to.join("Link.db"), "older").unwrap();
        let copied = freshen(&Tree { as_files: &link, ..Tree::new(&from, &to, &to) }).unwrap();
        assert_eq!(fs::read_to_string(to.join("Link.db")).unwrap(), "shared");
        assert_eq!(copied.as_files, link);
        // Any other link where a copy the move made has a file means a real file the pass before copied turned into a
        // link since, even one leading to a file of the same size and time: the copy stops.
        let what = |result: Result<Copied, CantMove>| match result {
            Err(CantMove::Failed(why)) => why,
            other => panic!("{:?}", other.map(|c| c.bytes)),
        };
        let why = what(freshen(&Tree::new(&from, &to, &to)));
        assert!(why.contains("Link.db turned into a link while it was being copied"), "{why}");
        // In a folder of yours, what's there stays.
        let keep_in = dir.join("Kept");
        assert!(freshen(&Tree { keep_in: Some(&keep_in), ..Tree::new(&from, &to, &to) }).is_ok());
        assert_eq!(fs::read_to_string(to.join("Link.db")).unwrap(), "shared");
        // One copied as its file that leads to no file now can't be taken as it is now: the copy stops.
        fs::remove_file(&shared).unwrap();
        let why = what(freshen(&Tree { as_files: &link, ..Tree::new(&from, &to, &to) }));
        assert!(why.contains("Link.db changed while it was being copied"), "{why}");
        assert_eq!(fs::read_to_string(to.join("Link.db")).unwrap(), "shared");
        let _ = fs::remove_dir_all(&dir);
    }

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
