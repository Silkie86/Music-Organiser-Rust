//! What can happen to moved folders afterwards, and putting it right: the C# app's checks on moved folders (lost,
//! replaced by an update, doubled by a later move), linking again, and moving the folder the moved folders live in
//! ("Plugin Data") as a whole. Like the move itself, nothing here deletes anything of yours: what's set aside goes only
//! when you say, and only to the Recycle Bin.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_formats::paths::file_name;
use mo_library::scan::is_inside;
use serde::{Deserialize, Serialize};

use crate::clean::{Bin, Done, Problem, What, on_network, send};
use crate::folder_move::{
    CantMove, Drives, Fill, HEADROOM, MovedFolder, Tree, check_rename, clear_failed_link, copy_tree, denied,
    inside_moved, link, only_in, remove_copy, remove_link, swap, target,
};
use crate::plugin_space::{PluginSpace, SpaceKind};
use crate::size::{self, is_link};
use crate::{key, real_path};

/// The folder a plugin update made where the link was, once it's set aside and the link is back.
pub const UPDATE_SUFFIX: &str = " (after update)";

/// The second copy a later move of the folder around it made, once it's set aside and the link is back.
pub const SECOND_SUFFIX: &str = " (second copy)";

/// Where linking again keeps the copy's own files that were newer than the ones replacing them, beside the copy.
pub const KEPT_BEFORE_SUFFIX: &str = " (newer files from before)";

/// A folder left behind for you to delete once the plugins open fine: the old place of the folder the moved folders
/// live in, after it moved to another drive, or the folder an update made where the link was (or a second copy a later
/// move made there), once its files joined the moved copy. Kept as the C# app keeps it (`library.db` setting
/// `space_old_homes`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct OldHome {
    pub path: String,
    pub bytes: u64,
    /// What it is ("its old place", "left by an update", "a second copy").
    #[serde(default = "its_old_place")]
    pub note: String,
    /// Where its files went, to check nothing is only in it before it goes. Rust only: the C# app leaves it out (and
    /// drops it when it saves the list), and then there's nothing to check it against.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub copy: String,
}

fn its_old_place() -> String {
    "its old place".to_owned()
}

/// The folders left behind, kept as the C# app keeps them, or none when they can't be read.
pub fn parse_old_homes(json: &str) -> Vec<OldHome> {
    serde_json::from_str(json).unwrap_or_default()
}

/// The folders left behind as the C# app keeps them.
pub fn old_homes_json(homes: &[OldHome]) -> String {
    serde_json::to_string(homes).unwrap_or_else(|_| "[]".to_owned())
}

/// Where a moved folder lives: the folder picked when moving it.
pub fn home_of(moved: &MovedFolder) -> String {
    Path::new(&moved.to).parent().map_or_else(|| moved.to.clone(), |p| p.to_string_lossy().into_owned())
}

/// The moved folders whose link leads nowhere: their folder on the other drive was moved or renamed, or the drive isn't
/// plugged in.
pub fn lost(moved: &[MovedFolder]) -> Vec<&MovedFolder> {
    moved.iter().filter(|m| is_link(Path::new(&m.from)) && !is_dir(&m.to)).collect()
}

/// The moved folders a plugin update put a real folder back in place of (some installers delete the folder and make it
/// again): the plugin writes on this drive again, and the copy on the other drive is left out of date. A folder inside
/// one moved later isn't one of these: see [`doubled`].
pub fn replaced(moved: &[MovedFolder]) -> Vec<&MovedFolder> {
    moved.iter().filter(|m| real_again(m) && inside_moved(&m.from, moved).is_none()).collect()
}

/// The moved folders that were inside a folder moved after them by the C# app before it kept links inside a folder as
/// links (F12 note, 27 Oct: Reaktor 6, then Native Instruments around it). That move copied what their links led to,
/// so the other drive has two copies: the plugin uses the one inside the later move, and the folder's own copy is left
/// out of date. Each comes with the moved folder around it. The Rust move keeps links as links, so it never makes these.
pub fn doubled(moved: &[MovedFolder]) -> Vec<(&MovedFolder, String)> {
    moved.iter().filter(|m| real_again(m)).filter_map(|m| Some((m, inside_moved(&m.from, moved)?.to_owned()))).collect()
}

/// A real folder where the link was, and the copy still there.
fn real_again(m: &MovedFolder) -> bool {
    is_dir(&m.from) && !is_link(Path::new(&m.from)) && is_dir(&m.to)
}

fn is_dir(path: &str) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_dir())
}

/// Where a path inside a moved folder really is (Reaktor 6's second copy, inside Native Instruments, is on the other
/// drive), so what's set aside in there goes to that drive's Recycle Bin.
pub fn real_place(path: &str, moved: &[MovedFolder]) -> String {
    let Some(outer) = inside_moved(path, moved) else { return path.to_owned() };
    let Some(m) = moved.iter().find(|m| key(&m.from) == key(outer)) else { return path.to_owned() };
    match path.get(outer.trim_end_matches(['/', '\\']).len()..) {
        Some(rest) => Path::new(&m.to).join(rest.trim_start_matches(['/', '\\'])).to_string_lossy().into_owned(),
        None => path.to_owned(),
    }
}

/// Fills in the other plugins each moved folder was shared with, for moves made before that was kept (F12 note, 27
/// Oct: EZkeys 2 still showed Toontrack on C: after it moved). Their sizes from before the move still list the folder,
/// so those plugins are also the ones to measure again (the second list). A folder with sharers already keeps them.
pub fn find_sharers(moved: &[MovedFolder], spaces: &[PluginSpace]) -> (Vec<MovedFolder>, Vec<String>) {
    let mut stale: Vec<String> = Vec::new();
    let now = moved
        .iter()
        .map(|m| {
            if !m.shared_with.is_empty() {
                return m.clone();
            }
            let sharers: Vec<String> = spaces
                .iter()
                .filter(|s| s.key != m.key && s.folders.iter().any(|f| key(&f.path) == key(&m.from)))
                .map(|s| s.key.clone())
                .collect();
            stale.extend(sharers.iter().cloned());
            MovedFolder { shared_with: sharers, ..m.clone() }
        })
        .collect();
    stale.sort();
    stale.dedup();
    (now, stale)
}

/// After an update replaced the link with a real folder (or, with `second`, a later move left a second copy where the
/// link was): the folder's new files, and files that differ, join the copy on the other drive (the copy's own newer
/// files are kept beside it in "… (newer files from before)"), then it's set aside as "… (after update)" (or "… (second
/// copy)") and the link goes back. Only with room for it on the other drive. Returns the folder set aside, at its real
/// place, to delete once the plugin opens fine. `all` is every moved folder. Close the DAW first. Call it away from the
/// window's thread.
pub fn link_again(
    moved: &MovedFolder,
    all: &[MovedFolder],
    bytes: u64,
    second: bool,
    drives: &dyn Drives,
    progress: &(dyn Fn(u64) + Sync),
    stop: &AtomicBool,
) -> Result<OldHome, CantMove> {
    let from = moved.from.trim_end_matches(['/', '\\']);
    let to = Path::new(&moved.to);
    if !is_dir(&moved.to) {
        return Err(CantMove::Failed(format!("{} isn't there any more.", moved.to)));
    }
    if is_link(Path::new(from)) || !is_dir(from) {
        return Err(CantMove::Failed(format!("{from} isn't a real folder now, so it was left alone.")));
    }
    let suffix = if second { SECOND_SUFFIX } else { UPDATE_SUFFIX };
    let mut aside = format!("{from}{suffix}");
    let mut n = 2;
    while fs::symlink_metadata(&aside).is_ok() {
        aside = format!("{from}{} {n})", suffix.trim_end_matches(')'));
        n += 1;
    }
    // Find out before copying whether Windows lets the folder be renamed.
    check_rename(from)?;
    let kept_in = PathBuf::from(format!("{}{KEPT_BEFORE_SUFFIX}", moved.to));
    let tree = Tree { from: Path::new(from), to, settled: to, keep_in: Some(&kept_in), leave_out: &[] };
    let needs = copy_tree(&tree, Fill::Count, &|_| {}, stop)?.bytes;
    if let Some(free) = drives.free(to).filter(|&free| free < needs.saturating_add(HEADROOM)) {
        return Err(CantMove::NoRoom { free, needs });
    }
    let first = copy_tree(&tree, Fill::Freshen, progress, stop)?;
    // What the plugin saved meanwhile.
    copy_tree(&tree, Fill::Freshen, &|n| progress(first.bytes + n), stop)?;
    swap(from, to, &aside)?;
    Ok(OldHome {
        path: real_place(&aside, all),
        bytes,
        note: if second { "a second copy" } else { "left by an update" }.to_owned(),
        copy: moved.to.clone(),
    })
}

/// Whether a folder left behind can go: it's there, and no moved folder (or its link) is still in it. After the folder
/// the moved folders live in moved, a link that couldn't be pointed at the new place still leads into the old one.
pub fn old_home_can_go(home: &OldHome, moved: &[MovedFolder]) -> bool {
    is_dir(&home.path) && !moved.iter().any(|m| is_inside(&m.to, &home.path) || is_inside(&m.from, &home.path))
}

/// What's in a folder left behind but not where its files went, or newer in it, as paths inside it. Nothing when it
/// doesn't say where its files went (one the C# app kept). Reads both folders: call it away from the window's thread.
pub fn only_in_old_home(home: &OldHome, stop: &AtomicBool) -> Result<Vec<String>, CantMove> {
    if home.copy.is_empty() {
        return Ok(Vec::new());
    }
    if !is_dir(&home.copy) {
        return Ok(vec![home.copy.clone()]);
    }
    only_in(Path::new(&home.path), Path::new(&home.copy), stop)
}

/// Sends a folder left behind to the Recycle Bin, once [`old_home_can_go`] says it may, and, unless you're `sure`, only
/// when nothing is only in it ([`only_in_old_home`]: the app shows that list first). [`crate::clean::undo`] puts it
/// back.
pub fn remove_old_home(home: &OldHome, moved: &[MovedFolder], sure: bool, bin: &dyn Bin, stop: &AtomicBool) -> Done {
    let stays = |why: &str| Done {
        problems: vec![Problem { path: home.path.clone(), why: why.to_owned() }],
        ..Done::default()
    };
    if !old_home_can_go(home, moved) {
        return stays("A moved folder, or its link, is still in it, so it stays.");
    }
    if !sure {
        match only_in_old_home(home, stop) {
            Err(_) => return stays("Stopped before it was all checked, so it stays."),
            Ok(only) if !only.is_empty() => {
                let first: Vec<&str> = only.iter().take(5).map(String::as_str).collect();
                let more = if only.len() > first.len() {
                    format!(" and {} more", only.len() - first.len())
                } else {
                    String::new()
                };
                return stays(&format!(
                    "Some of it isn't where its files went, or is newer here: {}{more}. Look before deleting it.",
                    first.join(", ")
                ));
            }
            Ok(_) => {}
        }
    }
    let mut done = Done::default();
    let real = real_path(&home.path).unwrap_or_else(|| home.path.clone());
    send(&home.path, real, home.bytes, What::Plugin(SpaceKind::Sounds), bin, &mut done);
    let not_found = bin.find(&mut done.removed);
    done.problems.extend(not_found);
    done
}

/// What moving the folder the moved folders live in did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HomeMoved {
    /// The moved folders as they are now (the ones that lived there, at their new place).
    pub moved: Vec<MovedFolder>,
    /// Links that couldn't be pointed at the new place (where the link is, and why): they still lead to the old one.
    pub problems: Vec<Problem>,
    /// Its old place, when it was copied to another drive: left for you to delete once the plugins open fine.
    pub old_home: Option<OldHome>,
}

/// Moves the folder the moved folders live in (all of it, other files too) and points every link at the new place. On
/// the same drive it's renamed; to another drive it's copied and checked, and the old one stays for you to delete. The
/// app keeps the moves it returns. Close the DAW first. Call it away from the window's thread.
pub fn move_home(
    home: &str,
    destination: &str,
    moved: &[MovedFolder],
    bytes: u64,
    drives: &dyn Drives,
    progress: &(dyn Fn(u64) + Sync),
    stop: &AtomicBool,
) -> Result<HomeMoved, CantMove> {
    let home = home.trim_end_matches(['/', '\\']);
    let home_path = Path::new(home);
    if is_link(home_path) {
        return Err(CantMove::Link);
    }
    if !is_dir(home) {
        return Err(CantMove::Gone);
    }
    let real_home = real_path(home).unwrap_or_else(|| home.to_owned());
    let real_destination = real_path(destination).unwrap_or_else(|| destination.to_owned());
    if is_inside(destination, home) || is_inside(&real_destination, &real_home) {
        return Err(CantMove::DestinationInside);
    }
    if on_network(destination) || on_network(&real_destination) {
        return Err(CantMove::NetworkDrive);
    }
    if !is_dir(destination) {
        return Err(CantMove::DestinationGone);
    }
    let to = target(home, destination);
    if fs::symlink_metadata(&to).is_ok() {
        return Err(CantMove::TargetThere(to.to_string_lossy().into_owned()));
    }
    let new_home = to.to_string_lossy().into_owned();
    if drives.same(home_path, Path::new(destination)) {
        // One rename: nothing is copied, and nothing is left behind.
        check_rename(home)?;
        fs::rename(home, &to)
            .map_err(|e| if denied(&e) { CantMove::NeedsAdmin } else { CantMove::InUse(e.to_string()) })?;
        let (moved, problems) = repoint(home, &new_home, moved);
        return Ok(HomeMoved { moved, problems, old_home: None });
    }
    match size::has_online_only(home_path, stop) {
        Ok(true) => return Err(CantMove::OnlineOnly),
        Ok(false) => {}
        Err(_) => return Err(CantMove::Stopped),
    }
    if let Some(free) = drives.free(Path::new(destination)).filter(|&free| free < bytes.saturating_add(HEADROOM)) {
        return Err(CantMove::NoRoom { free, needs: bytes });
    }
    fs::create_dir(&to).map_err(|e| CantMove::Failed(format!("Couldn't make {new_home} ({e}).")))?;
    let tree = Tree { from: home_path, to: &to, settled: &to, keep_in: None, leave_out: &[] };
    let copied = copy_tree(&tree, Fill::New, progress, stop)
        .and_then(|first| copy_tree(&tree, Fill::Freshen, &|n| progress(first.bytes + n), stop));
    if let Err(why) = copied {
        remove_copy(&to);
        return Err(why);
    }
    let (moved, problems) = repoint(home, &new_home, moved);
    let old_home = OldHome { path: home.to_owned(), bytes, note: its_old_place(), copy: new_home };
    Ok(HomeMoved { moved, problems, old_home: Some(old_home) })
}

/// "I moved it myself": points the link of every moved folder that lived in `old_home` at the same folder in
/// `new_home`, where it's found. Returns the moves as they are now, and the links that couldn't be pointed there
/// (they're left as they were). Only ever changes links, never a real folder.
pub fn repoint(old_home: &str, new_home: &str, moved: &[MovedFolder]) -> (Vec<MovedFolder>, Vec<Problem>) {
    let old_home = old_home.trim_end_matches(['/', '\\']);
    let mut problems = Vec::new();
    let mut links: Vec<(usize, PathBuf)> = Vec::new();
    for (i, m) in moved.iter().enumerate() {
        if !is_inside(&m.to, old_home) {
            continue;
        }
        let Some(rest) = m.to.get(old_home.len()..) else { continue };
        let to = Path::new(new_home).join(rest.trim_start_matches(['/', '\\']));
        if is_dir(&to.to_string_lossy()) {
            links.push((i, to));
        } else {
            problems.push(Problem { path: m.from.clone(), why: format!("{} isn't in {new_home}.", file_name(&m.to)) });
        }
    }
    // Outer links first: a moved folder inside another (Reaktor 6 in Native Instruments) is reached through the outer
    // link, so it's pointed at the new place only once the outer one is.
    let around = |at: &str| {
        links.iter().filter(|(o, _)| key(&moved[*o].from) != key(at) && is_inside(at, &moved[*o].from)).count()
    };
    let mut order: Vec<(usize, usize)> =
        links.iter().enumerate().map(|(n, (i, _))| (around(&moved[*i].from), n)).collect();
    order.sort();
    let mut now = moved.to_vec();
    for (_, n) in order {
        let (i, to) = &links[n];
        match relink(&moved[*i].from, to) {
            Ok(()) => now[*i].to = to.to_string_lossy().into_owned(),
            Err(why) => problems.push(Problem { path: moved[*i].from.clone(), why }),
        }
    }
    (now, problems)
}

/// Swaps the link at `at` for one to `target`; makes one where there's nothing. Never touches a real folder. If the new
/// link doesn't take, the old one goes back.
fn relink(at: &str, target: &Path) -> Result<(), String> {
    let at_path = Path::new(at);
    if !is_dir(&target.to_string_lossy()) {
        return Err(format!("{} isn't there.", target.to_string_lossy()));
    }
    if !is_link(at_path) {
        if fs::symlink_metadata(at_path).is_ok() {
            return Err(format!("{at} is a real folder now, not a link, so it was left alone."));
        }
        return link(at_path, target).map_err(|e| {
            clear_failed_link(at_path);
            format!("Windows wouldn't make the link ({e}).")
        });
    }
    let was = fs::read_link(at_path).ok();
    remove_link(at_path).map_err(|e| {
        if denied(&e) {
            "Windows said no: the link needs administrator rights to change, which this version can't ask for yet."
                .to_owned()
        } else {
            format!("Windows wouldn't change the link ({e}).")
        }
    })?;
    let Err(e) = link(at_path, target) else { return Ok(()) };
    clear_failed_link(at_path);
    // The old link goes back, so the plugin finds what it found before.
    let back = was.is_some_and(|was| link(at_path, &was).is_ok());
    if back {
        Err(format!("Windows wouldn't make the link ({e}), so the old one is back."))
    } else {
        Err(format!(
            "Windows wouldn't make the link ({e}), nor put the old one back: make a link at {at} to {} yourself.",
            target.to_string_lossy()
        ))
    }
}
