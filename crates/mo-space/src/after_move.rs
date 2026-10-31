//! What can happen to moved folders afterwards, and putting it right: the C# app's checks on moved folders (lost,
//! replaced by an update, doubled), linking again, and moving the folder the moved folders live in ("Plugin Data") as
//! a whole. Like the move itself, nothing here deletes anything of yours: what's set aside goes only when you say, and
//! only to the Recycle Bin, and a link is only ever changed, never a real folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_formats::paths::file_name;
use mo_library::scan::is_inside;
use serde::{Deserialize, Serialize};

use crate::admin::{Admin, Step, ask_for, real_spot, reason};
use crate::clean::{Bin, Done, Problem, Sending, What};
use crate::folder_move::{
    CantMove, Drives, Fill, MovedFolder, Tree, check_rename, check_rename_or_ask, clear_failed_link, copy_fresh,
    copy_tree, denied, destination_problem, fresh_name, inside_moved, leads_to, link, link_reaches, listed, only_in,
    remove_link, rest_after, room_for, shown, swap_or_ask, target,
};
use crate::plugin_space::{PluginSpace, SpaceKind};
use crate::size::{self, is_link};
use crate::{is_dir, key, real_path, text};

/// The folder a plugin update made where the link was, once it's set aside and the link is back.
pub const UPDATE_SUFFIX: &str = " (after update)";

/// The second copy a later move of the folder around it made, once it's set aside and the link is back.
pub const SECOND_SUFFIX: &str = " (second copy)";

/// Where linking again keeps the files in the copy that it replaces, beside the copy.
pub const KEPT_BEFORE_SUFFIX: &str = " (kept from before)";

/// A folder left behind for you to delete once the plugins open fine: the old place of the folder the moved folders
/// live in, after it moved to another drive; the folder an update made where the link was (or a second copy a later
/// move made there), once its files joined the moved copy; or the files linking again replaced. Kept as the C# app keeps
/// it (`library.db` setting `space_old_homes`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct OldHome {
    pub path: String,
    pub bytes: u64,
    /// What it is ("its old place", "left by an update", "a second copy", "replaced by linking again").
    #[serde(default = "its_old_place")]
    pub note: String,
    /// Where its files went, to check nothing is only in it before it goes. Rust only (the C# app leaves it out, and
    /// drops it when it saves the list): without it there's nothing to check against, so it goes only once you're sure.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub copy: String,
    /// What it holds, as [`MovedFolder::kind`] numbers it (Rust only; sounds when it isn't said).
    #[serde(default = "sounds_number", skip_serializing_if = "is_sounds")]
    pub kind: u8,
}

fn its_old_place() -> String {
    "its old place".to_owned()
}

fn sounds_number() -> u8 {
    2
}

fn is_sounds(kind: &u8) -> bool {
    *kind == 2
}

impl OldHome {
    fn space_kind(&self) -> SpaceKind {
        match self.kind {
            0 => SpaceKind::Program,
            1 => SpaceKind::Presets,
            _ => SpaceKind::Sounds,
        }
    }
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

/// The moved folders with a real folder where their link was, inside a folder moved after them, each with that moved
/// folder. Mostly a second copy the C# app's older move made (before it kept links inside a folder as links, F12 note,
/// 27 Oct: Reaktor 6, then Native Instruments around it), so the plugin uses that one and the folder's own copy is left
/// out of date; it can also be an update that replaced the link in there. Either way [`link_again`] puts it right.
pub fn doubled(moved: &[MovedFolder]) -> Vec<(&MovedFolder, String)> {
    moved.iter().filter(|m| real_again(m)).filter_map(|m| Some((m, inside_moved(&m.from, moved)?.to_owned()))).collect()
}

/// A real folder where the link was, and the copy still there.
fn real_again(m: &MovedFolder) -> bool {
    is_dir(&m.from) && !is_link(Path::new(&m.from)) && is_dir(&m.to)
}

/// Where a path inside a moved folder really is (Reaktor 6's second copy, inside Native Instruments, is on the other
/// drive), so what's set aside in there goes to that drive's Recycle Bin.
pub fn real_place(path: &str, moved: &[MovedFolder]) -> String {
    let Some(outer) = inside_moved(path, moved) else { return path.to_owned() };
    let Some(m) = moved.iter().find(|m| key(&m.from) == key(outer)) else { return path.to_owned() };
    rest_after(path, outer).map_or_else(|| path.to_owned(), |rest| text(&Path::new(&m.to).join(rest)))
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

/// What linking again did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedAgain {
    /// The folder set aside, at its real place, to delete once the plugin opens fine.
    pub set_aside: OldHome,
    /// Files in the copy that were replaced (paths inside it), kept in `kept`.
    pub replaced: Vec<String>,
    /// Where they're kept, beside the copy, when any were: for you to look at, and delete once you're happy.
    pub kept: Option<OldHome>,
    /// Links in the copy where the folder had a file or folder (paths inside it): left as they were.
    pub links_left: Vec<String>,
}

/// After an update replaced the link with a real folder (or, with `second`, a later move left a second copy where the
/// link was): the folder's new files, and files that differ, join the copy on the other drive, then it's set aside as
/// "… (after update)" (or "… (second copy)") and the link goes back. Every file in the copy it replaces is kept first in
/// "… (kept from before)" beside the copy (the update's file may be the newer one, but yours was the one in use). Only
/// with room for it on the other drive. `all` is every moved folder. Close the DAW first. Call it away from the
/// window's thread.
#[allow(clippy::too_many_arguments)]
pub fn link_again(
    moved: &MovedFolder,
    all: &[MovedFolder],
    bytes: u64,
    second: bool,
    drives: &dyn Drives,
    progress: &(dyn Fn(u64) + Sync),
    stop: &AtomicBool,
) -> Result<LinkedAgain, CantMove> {
    let from = moved.from.trim_end_matches(['/', '\\']);
    let to = Path::new(&moved.to);
    if !is_dir(&moved.to) {
        return Err(CantMove::Failed(format!("{} isn't there any more.", moved.to)));
    }
    if is_link(Path::new(from)) || !is_dir(from) {
        return Err(CantMove::Failed(format!("{from} isn't a real folder now, so it was left alone.")));
    }
    let aside = fresh_name(from, if second { SECOND_SUFFIX } else { UPDATE_SUFFIX });
    // Find out before copying whether Windows lets the folder be renamed.
    check_rename_or_ask(from, drives.admin())?;
    let kept_in = PathBuf::from(fresh_name(&moved.to, KEPT_BEFORE_SUFFIX));
    let tree = Tree { keep_in: Some(&kept_in), keep_all: true, ..Tree::new(Path::new(from), to, to) };
    room_for(&tree, to, drives, stop)?;
    // From here on the copy changes: a stop or a failure says so.
    let partly = |why: CantMove| match why {
        CantMove::LeftHalfway(_) => why,
        other => {
            let said = match other {
                CantMove::Stopped => "Stopped.".to_owned(),
                // A folder took the name it was to be set aside under meanwhile.
                CantMove::OldCopyThere => {
                    format!("There's a folder called {} beside it now: rename or delete it.", file_name(&aside))
                }
                other => other.text(),
            };
            let kept = if kept_in.exists() {
                format!(", and the ones they replaced are kept in {}", text(&kept_in))
            } else {
                String::new()
            };
            CantMove::LeftHalfway(format!(
                "{} Some of its files may be in {} already{kept}. The folder here is as it was, so it's safe to try \
                 again once the DAW is closed.",
                said.trim_end_matches("Nothing was moved.").trim_end(),
                moved.to
            ))
        }
    };
    let first = copy_tree(&tree, Fill::Freshen, progress, stop).map_err(partly)?;
    // What the plugin saved meanwhile. What the first pass wrote is the update's, not yours: it isn't kept again.
    let mut ours = first.written.clone();
    ours.sort();
    let again = copy_tree(&Tree { ours: &ours, ..tree }, Fill::Freshen, &|n| progress(first.bytes + n), stop)
        .map_err(partly)?;
    swap_or_ask(from, to, &aside, drives.admin()).map_err(partly)?;
    let replaced = shown(&[first.kept, again.kept].concat());
    let kept = (!replaced.is_empty()).then(|| OldHome {
        path: text(&kept_in),
        bytes: size::measure(&kept_in, stop).map_or(0, |m| m.bytes),
        note: "replaced by linking again".to_owned(),
        copy: String::new(),
        kind: moved.kind,
    });
    Ok(LinkedAgain {
        set_aside: OldHome {
            path: real_place(&aside, all),
            bytes,
            note: if second { "a second copy" } else { "left by an update" }.to_owned(),
            copy: moved.to.clone(),
            kind: moved.kind,
        },
        replaced,
        kept,
        links_left: shown(&[first.links_left, again.links_left].concat()),
    })
}

/// The folders of files linking again kept from before ("… (kept from before)", beside each moved folder's copy) that
/// aren't in `homes`: one a try that stopped partway left is found too, so none of your files are where the app doesn't
/// show them. Measures them: call it away from the window's thread.
pub fn kept_beside(moved: &[MovedFolder], homes: &[OldHome], stop: &AtomicBool) -> Vec<OldHome> {
    let known = |path: &Path| homes.iter().any(|h| key(&h.path) == key(&text(path)));
    let mut found: Vec<OldHome> = Vec::new();
    for m in moved {
        let to = Path::new(&m.to);
        let (Some(parent), Some(name)) = (to.parent(), to.file_name()) else { continue };
        let start = format!("{}{}", name.to_string_lossy(), KEPT_BEFORE_SUFFIX.trim_end_matches(')'));
        let Ok(entries) = fs::read_dir(parent) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let entry_name = entry.file_name().to_string_lossy().into_owned();
            let numbered = entry_name.strip_prefix(&start).and_then(|rest| rest.strip_suffix(')')).is_some_and(|n| {
                n.is_empty()
                    || n.strip_prefix(' ').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            });
            if !numbered || !is_dir(&text(&path)) || is_link(&path) || known(&path) {
                continue;
            }
            if found.iter().any(|h| key(&h.path) == key(&text(&path))) {
                continue;
            }
            found.push(OldHome {
                path: text(&path),
                bytes: size::measure(&path, stop).map_or(0, |m| m.bytes),
                note: "replaced by linking again".to_owned(),
                copy: String::new(),
                kind: m.kind,
            });
        }
    }
    found
}

/// Whether a folder left behind can go: it's there, and no moved folder (or its link) is still in it. After the folder
/// the moved folders live in moved, a link that couldn't be pointed at the new place still leads into the old one.
pub fn old_home_can_go(home: &OldHome, moved: &[MovedFolder]) -> bool {
    is_dir(&home.path) && !moved.iter().any(|m| is_inside(&m.to, &home.path) || is_inside(&m.from, &home.path))
}

/// What's in a folder left behind but not where its files went, or newer in it, as paths inside it; `None` when it
/// doesn't say where its files went (the C# app's, or the files linking again replaced), so it can't be told. Reads
/// both folders: call it away from the window's thread.
pub fn only_in_old_home(home: &OldHome, stop: &AtomicBool) -> Result<Option<Vec<String>>, CantMove> {
    if home.copy.is_empty() {
        return Ok(None);
    }
    if !is_dir(&home.copy) {
        return Ok(Some(vec![home.copy.clone()]));
    }
    only_in(Path::new(&home.path), Path::new(&home.copy), stop).map(Some)
}

/// Sends a folder left behind to the Recycle Bin, once [`old_home_can_go`] says it may, and, unless you're `sure`, only
/// when nothing is only in it ([`only_in_old_home`]: the app shows that list first; one it can't tell goes only once
/// you're sure). [`crate::clean::undo`] puts it back.
pub fn remove_old_home(home: &OldHome, moved: &[MovedFolder], sure: bool, bin: &dyn Bin, stop: &AtomicBool) -> Done {
    let stays = |why: &str| Done {
        problems: vec![Problem { path: home.path.clone(), why: why.to_owned() }],
        ..Done::default()
    };
    if !is_dir(&home.path) {
        return stays("It isn't there any more.");
    }
    if !old_home_can_go(home, moved) {
        return stays("A moved folder, or its link, is still in it, so it stays.");
    }
    if !sure {
        match only_in_old_home(home, stop) {
            Err(_) => return stays("Stopped before it was all checked, so it stays."),
            Ok(None) => {
                return stays(
                    "There's nothing to check it against, so it goes only once you've looked in it and say you're \
                     sure.",
                );
            }
            Ok(Some(only)) if !only.is_empty() => {
                return stays(&format!(
                    "Some of it isn't where its files went, or is newer here: {}. Look before deleting it.",
                    listed(&only)
                ));
            }
            Ok(Some(_)) => {}
        }
    }
    let mut done = Done::default();
    let real = real_path(&home.path).unwrap_or_else(|| home.path.clone());
    // Checked just now; one Windows guards is checked again by the administrator step, against what it held now.
    let mut sending = Sending::new(bin);
    sending.send(&home.path, real, home.bytes, What::Plugin(home.space_kind()), &mut done, || Ok(()));
    sending.finish(&mut done);
    done
}

/// What moving the folder the moved folders live in (or pointing links at it after you moved it) did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HomeMoved {
    /// The moved folders as they are now: the ones whose copy is at the new place point there.
    pub moved: Vec<MovedFolder>,
    /// The folders left behind as they are now: the ones that were in it are at the new place.
    pub old_homes: Vec<OldHome>,
    /// Links that couldn't be pointed at the new place (where the link is, and why), and moved folders whose copy
    /// isn't there: they're as they were.
    pub problems: Vec<Problem>,
    /// Its old place, when it was copied to another drive: left for you to delete once the plugins open fine.
    pub old_home: Option<OldHome>,
}

/// Moves the folder the moved folders live in (all of it, other files too) and points every link at the new place. On
/// the same drive it's one rename, with the links pointed at the new place just before (if any won't change, or the
/// rename fails, they're put back and nothing moves). To another drive it's copied and checked, and the old one stays
/// for you to delete. `homes` is every folder left behind. The app keeps the moves and homes it returns. Close the DAW
/// first. Call it away from the window's thread.
#[allow(clippy::too_many_arguments)]
pub fn move_home(
    home: &str,
    destination: &str,
    moved: &[MovedFolder],
    homes: &[OldHome],
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
    if let Some(why) = destination_problem(home, destination, bytes, drives, true, stop) {
        return Err(why);
    }
    let to = target(home, destination);
    let new_home = text(&to);
    if drives.same(home_path, Path::new(destination)) {
        check_rename(home).map_err(rename_it_yourself)?;
        return rename_home(home, &to, moved, homes).map_err(rename_it_yourself);
    }
    copy_fresh(&Tree::new(home_path, &to, &to), progress, stop)?;
    let now = repoint(home, &new_home, moved, homes, drives.admin());
    let old_home = OldHome { path: home.to_owned(), bytes, note: its_old_place(), copy: new_home, kind: 2 };
    Ok(HomeMoved { old_home: Some(old_home), ..now })
}

/// Renaming the folder the moved folders live in isn't done as administrator (it's yours, on your drive, so Windows
/// rarely guards it): when Windows says no, File Explorer can do it, and then the links are pointed at it here.
fn rename_it_yourself(why: CantMove) -> CantMove {
    match why {
        CantMove::NeedsAdmin => CantMove::Failed(
            "Windows said no to renaming it. Close your DAW, the plugins and any window showing the folder, then try \
             again. If Windows still says no, rename the folder in File Explorer, then point the links at it here."
                .to_owned(),
        ),
        other => other,
    }
}

/// The moved folders whose copy is in `old_home`, with where it is in `new_home`.
fn living_in(old_home: &str, new_home: &str, moved: &[MovedFolder]) -> Vec<(usize, PathBuf)> {
    moved
        .iter()
        .enumerate()
        .filter_map(|(i, m)| Some((i, Path::new(new_home).join(rest_after(&m.to, old_home)?))))
        .collect()
}

/// The links (by their place in `links`) level by level, outer ones first: a moved folder inside another (Reaktor 6 in
/// Native Instruments) is reached through the outer link.
fn levels(links: &[(usize, PathBuf)], moved: &[MovedFolder]) -> Vec<Vec<usize>> {
    let around = |at: &str| {
        links.iter().filter(|(o, _)| key(&moved[*o].from) != key(at) && is_inside(at, &moved[*o].from)).count()
    };
    let mut levels: Vec<Vec<usize>> = Vec::new();
    for (n, (i, _)) in links.iter().enumerate() {
        let level = around(&moved[*i].from);
        if levels.len() <= level {
            levels.resize_with(level + 1, Vec::new);
        }
        levels[level].push(n);
    }
    levels.retain(|l| !l.is_empty());
    levels
}

/// The order to change links in: outer ones first.
fn outer_first(links: &[(usize, PathBuf)], moved: &[MovedFolder]) -> Vec<usize> {
    levels(links, moved).concat()
}

/// The folders left behind that were in `old_home`, at their place in `new_home` when they're there.
fn follow_homes(old_home: &str, new_home: &str, homes: &[OldHome]) -> Vec<OldHome> {
    let moved_to = |path: &str| {
        rest_after(path, old_home)
            .map(|rest| text(&Path::new(new_home).join(rest)))
            .filter(|p| fs::symlink_metadata(p).is_ok())
    };
    homes
        .iter()
        .map(|h| OldHome {
            path: moved_to(&h.path).unwrap_or_else(|| h.path.clone()),
            copy: moved_to(&h.copy).unwrap_or_else(|| h.copy.clone()),
            ..h.clone()
        })
        .collect()
}

/// The same-drive move: links first, to where the copies will be, then one rename. If a link won't change, or the
/// rename fails, the links changed are put back and nothing has moved.
fn rename_home(home: &str, to: &Path, moved: &[MovedFolder], homes: &[OldHome]) -> Result<HomeMoved, CantMove> {
    let new_home = text(to);
    // Only working links are changed: one that leads nowhere, or a real folder, is left as it is.
    let links: Vec<(usize, PathBuf)> =
        living_in(home, &new_home, moved).into_iter().filter(|(i, _)| link_reaches(&moved[*i])).collect();
    let mut changed: Vec<usize> = Vec::new();
    // Put back in the opposite order, so an outer link leads to its folder again before the links inside it change.
    let put_back = |changed: &[usize]| -> String {
        let mut stuck = Vec::new();
        for &n in changed.iter().rev() {
            let m = &moved[links[n].0];
            if relink(&m.from, Path::new(&m.to), true).is_err() {
                stuck.push(format!("make a link at {} to {}", m.from, m.to));
            }
        }
        if stuck.is_empty() {
            String::new()
        } else {
            format!(" Some links couldn't be put back: {}.", stuck.join("; "))
        }
    };
    // Inner links first: while the folder is still where it was, a link inside another moved folder (Reaktor 6 in
    // Native Instruments) is only reached through the outer link as it is now.
    let mut order = outer_first(&links, moved);
    order.reverse();
    for n in order {
        let (i, new_to) = &links[n];
        if let Err(not) = relink(&moved[*i].from, new_to, false) {
            let stuck = put_back(&changed);
            let why = if not.guarded {
                "Windows guards it. Rename the folder in File Explorer instead, then point the links at it there (the \
                 app asks Windows' administrator question for the ones it guards)."
                    .to_owned()
            } else {
                not.why
            };
            let said = format!("The link at {} couldn't be pointed at the new place: {why}{stuck}", moved[*i].from);
            return Err(if not.gone || !stuck.is_empty() {
                CantMove::LeftHalfway(format!("{said} The folder hasn't moved."))
            } else {
                CantMove::Failed(said)
            });
        }
        changed.push(n);
    }
    if let Err(e) = fs::rename(home, to) {
        let stuck = put_back(&changed);
        if !stuck.is_empty() {
            return Err(CantMove::LeftHalfway(format!(
                "Windows wouldn't rename {home} ({e}), so it stays where it was.{stuck}"
            )));
        }
        return Err(if denied(&e) { CantMove::NeedsAdmin } else { CantMove::InUse(e.to_string()) });
    }
    // Every link changed reaches the folder at its new place, or it all goes back.
    if let Some(&n) = changed.iter().find(|&&n| !leads_to(&moved[links[n].0].from, &links[n].1)) {
        let at = &moved[links[n].0].from;
        if let Err(e) = fs::rename(to, home) {
            // It stays at the new place, and so do the links that reach it there.
            return Err(CantMove::LeftHalfway(format!(
                "The link at {at} didn't reach the folder at its new place, and Windows wouldn't rename it back \
                 ({e}). It's at {new_home} now, with the links pointed there: its moved folders show as lost until \
                 they're pointed at it there."
            )));
        }
        let stuck = put_back(&changed);
        return Err(if stuck.is_empty() {
            CantMove::Failed(format!(
                "The link at {at} didn't reach the folder at its new place, so it was all put back."
            ))
        } else {
            CantMove::LeftHalfway(format!(
                "The link at {at} didn't reach the folder at its new place, so the folder went back.{stuck}"
            ))
        });
    }
    // Every moved folder whose copy moved with it follows, the ones whose link wasn't changed too.
    let mut now = moved.to_vec();
    let mut problems = Vec::new();
    for (i, new_to) in living_in(home, &new_home, moved) {
        if is_dir(&text(&new_to)) {
            now[i].to = text(&new_to);
        } else {
            problems.push(Problem { path: moved[i].from.clone(), why: format!("{} isn't there.", moved[i].to) });
        }
    }
    Ok(HomeMoved { moved: now, old_homes: follow_homes(home, &new_home, homes), problems, old_home: None })
}

/// "I moved it myself" (or the copy step of [`move_home`]): points the link of every moved folder that lived in
/// `old_home` at the same folder in `new_home`, where it's found. A real folder where a link was is left alone, but its
/// move follows its copy (so it shows as replaced). A link that won't change is left as it was; the ones Windows guards
/// are changed as administrator, Windows asking you once (`admin`: the app passes [`crate::admin::AsAdmin`]). `homes`
/// is every folder left behind. Only ever changes links, never a real folder.
pub fn repoint(
    old_home: &str,
    new_home: &str,
    moved: &[MovedFolder],
    homes: &[OldHome],
    admin: &dyn Admin,
) -> HomeMoved {
    let old_home = old_home.trim_end_matches(['/', '\\']);
    let mut problems = Vec::new();
    let mut now = moved.to_vec();
    let mut links = Vec::new();
    for (i, new_to) in living_in(old_home, new_home, moved) {
        let m = &moved[i];
        if is_dir(&text(&new_to)) {
            links.push((i, new_to));
        } else {
            problems.push(Problem { path: m.from.clone(), why: format!("{} isn't in {new_home}.", file_name(&m.to)) });
        }
    }
    // Outer links first: one inside another moved folder is only reached once the outer link leads somewhere again.
    // So the ones Windows guards are changed level by level too, before the next level is looked at.
    for level in levels(&links, moved) {
        let mut guarded: Vec<usize> = Vec::new();
        for n in level {
            let (i, new_to) = &links[n];
            let from = &moved[*i].from;
            if !is_link(Path::new(from)) && is_dir(from) {
                now[*i].to = text(new_to);
                continue;
            }
            match relink(from, new_to, true) {
                Ok(()) => now[*i].to = text(new_to),
                Err(not) if not.guarded && !not.gone && admin.can_ask() => guarded.push(n),
                Err(not) => problems.push(Problem { path: from.clone(), why: not.why }),
            }
        }
        if guarded.is_empty() {
            continue;
        }
        let lines: Vec<Vec<String>> =
            guarded.iter().map(|&n| vec![real_spot(&moved[links[n].0].from), text(&links[n].1)]).collect();
        let said = ask_for(admin, Step::Relink, &lines);
        for (k, &n) in guarded.iter().enumerate() {
            let (i, new_to) = &links[n];
            let from = &moved[*i].from;
            // What's there now decides, whatever the step said.
            if leads_to(from, new_to) {
                now[*i].to = text(new_to);
                continue;
            }
            let otherwise = format!("After the administrator step, the link still doesn't lead to {}.", text(new_to));
            let why = reason(&said, k, &otherwise);
            problems.push(Problem { path: from.clone(), why });
        }
    }
    HomeMoved { moved: now, old_homes: follow_homes(old_home, new_home, homes), problems, old_home: None }
}

/// Why a link wasn't pointed at the new place.
pub(crate) struct NotRelinked {
    pub(crate) why: String,
    /// The old link couldn't be put back either: there's no link there now.
    pub(crate) gone: bool,
    /// Windows guards it: it needs administrator rights to change. The old link (or nothing) is still there.
    pub(crate) guarded: bool,
}

impl NotRelinked {
    fn kept(why: String) -> Self {
        NotRelinked { why, gone: false, guarded: false }
    }
}

/// What a link Windows guards says, where its administrator question can't be asked.
const GUARDED: &str = "Windows said no: the link needs administrator rights to change.";

/// Swaps the link at `at` for one to `target`; makes one where there's nothing. Never touches a real folder. With
/// `check`, the target must be there and the new link must reach it. If the new link doesn't take, the old one goes
/// back.
pub(crate) fn relink(at: &str, target: &Path, check: bool) -> Result<(), NotRelinked> {
    let at_path = Path::new(at);
    if check && !is_dir(&text(target)) {
        return Err(NotRelinked::kept(format!("{} isn't there.", text(target))));
    }
    let reaches = || !check || leads_to(at, target);
    if !is_link(at_path) {
        if fs::symlink_metadata(at_path).is_ok() {
            return Err(NotRelinked::kept(format!("{at} is a real folder now, not a link, so it was left alone.")));
        }
        let made = link(at_path, target);
        if made.is_ok() && reaches() {
            return Ok(());
        }
        clear_failed_link(at_path);
        let guarded = made.as_ref().is_err_and(denied);
        return Err(NotRelinked {
            why: match made {
                Err(_) if guarded => GUARDED.to_owned(),
                Err(e) => format!("Windows wouldn't make the link ({e})."),
                Ok(()) => format!("The link was made but doesn't reach {}.", text(target)),
            },
            gone: false,
            guarded,
        });
    }
    let was = fs::read_link(at_path).ok();
    remove_link(at_path).map_err(|e| {
        if denied(&e) {
            NotRelinked { why: GUARDED.to_owned(), gone: false, guarded: true }
        } else {
            NotRelinked::kept(format!("Windows wouldn't change the link ({e})."))
        }
    })?;
    let made = link(at_path, target);
    if made.is_ok() && reaches() {
        return Ok(());
    }
    clear_failed_link(at_path);
    let why = match made {
        Err(e) => format!("Windows wouldn't make the link ({e})"),
        Ok(()) => format!("The link was made but doesn't reach {}", text(target)),
    };
    // The old link goes back, so the plugin finds what it found before.
    if was.as_ref().is_some_and(|was| link(at_path, was).is_ok()) {
        return Err(NotRelinked::kept(format!("{why}, so the old one is back.")));
    }
    // Where a link there should lead: the new place when the folder is there, or else where it led before.
    let lead = if is_dir(&text(target)) { text(target) } else { was.as_deref().map_or_else(|| text(target), text) };
    Err(NotRelinked {
        why: format!("{why}, nor could the old one be put back: make a link at {at} to {lead} yourself."),
        gone: true,
        guarded: false,
    })
}
