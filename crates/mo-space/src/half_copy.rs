//! Half copies: what a move was copying when the app or the computer stopped (a crash, a power cut, the app closed
//! part way). Every copy a move makes as a new folder (moving a folder to another drive, moving it back once its old
//! copy is gone, moving the folder the moved folders live in) gets a marker the moment it's made: a small text file
//! inside it, [`MARKER`], saying what it's a copy of. Just before the copy goes into use (the swap for its link, or
//! pointing the links at it) the marker says so too, and once it's in use the marker comes out. So a copy that still
//! holds one was cut off part way (Solomon, 3 Nov 2026: yes to "should the move leave a marker so the app can clear
//! it?"). The app clears it then: the next try at the same move does that first, and the Space page finds the others
//! in the places moves go to ([`half_copies`]) and clears them ([`clear`]).
//!
//! Cleared means removed for good, as a move removes its copy when you press Stop: a half copy only ever holds copies
//! the move made. And only while all of this holds, looked at again just before:
//!
//! - nothing in this app is making or clearing a copy there now ([`claim`]);
//! - the marker is a plain file in the folder it names (the same name, when that place is gone: a drive with another
//!   letter now), and the folder is a real folder, not a link;
//! - the copy never started going into use: the plugin can't have saved into it;
//! - the folder it copies from is there, and doesn't lead to the copy;
//! - no moved folder's link leads to it or into it, and no moved folder's copy is it or inside it;
//! - moving back, the link is still in the folder's place, leading to what was being copied;
//! - everything in it has its twin in the folder it copies from, at the same place and of the same kind (a file for a
//!   file or a file link, a folder for a folder, a link for a link), and no file in it is newer than its twin (but one,
//!   no bigger: the file the copy was writing when it stopped). Left out: the marker, a file the copy was writing under
//!   its temporary name, and the files Explorer and Finder make by themselves.
//!
//! Otherwise it stays, and says why. A marker in a copy that's in use (the app stopped between the swap and taking the
//! marker out), or in a folder renamed since (a link at its old name, leading to it, doesn't make it the copy), is the
//! only thing that goes then: the folder is left as it is.
//!
//! Clearing stops when you press Stop, between the things it removes; the marker goes last, so what's left is still a
//! half copy, cleared the next time. A file Explorer or Finder writes into the emptied folder just then goes too; if
//! anything else is in it by then, the folder stays, with its marker. A copy whose swap fails (the swap didn't happen)
//! goes as a move's copy does when it stops, its marker saying first that it isn't going into use after all, so what
//! can't go then is cleared next time too.
//!
//! Removing never follows a link it meets: a link goes as the link, and each folder is looked at again just before
//! it's read. Something that swapped one of its folders for a link while it's being removed could still lead removals
//! through it: Rust's standard library can't remove things by a folder it holds open, as would close that. Nothing
//! does that to a half copy (a folder only the app makes, where moves go), so it's named here rather than guarded.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use mo_formats::paths::file_name;
use mo_library::scan::is_inside;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::folder_move::{
    CantMove, MovedFolder, OLD_SUFFIX, PART_SUFFIX, Tree, csharp_time, leads_to, listed, newer, remove_link, same_file,
    set_read_only,
};
use crate::plugin_clean::made_by_the_system;
use crate::size::{self, DEEPEST, Stopped, is_link};
use crate::{in_places, is_dir, key, not_there, parent_of, real_folder, real_path, text};

/// The marker's name, at the top of a copy a move is making.
pub const MARKER: &str = "Music Organiser - copy not finished.txt";

/// What the marker says first, for anyone who opens it.
const EXPLAINED: &str = "Music Organiser is copying a folder into this one. If this file is still here once the app \
                         has finished, the copy was cut off part way (the app or the computer stopped), so this folder \
                         is a half copy: the app clears it the next time it moves the folder, or from the Space page. \
                         The folder it copies from wasn't changed.";

/// The biggest marker read: anything bigger isn't one the app wrote.
const MARKER_MOST: u64 = 64 << 10;

/// What's said of a copy this app is making or clearing now.
const BUSY: &str = "The app is making or clearing a copy here now, so it stays.";

/// What a marker holds, after [`EXPLAINED`]: one line of JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Marker {
    /// The folder being copied.
    from: String,
    /// Where the copy is made: the folder holding the marker.
    to: String,
    /// Where the copy ends up: `to` itself, or, moving back, the folder's own place once the link has gone.
    settled: String,
    /// When the copy started, as the app writes times.
    started: String,
    /// Whether the copy started going into use (the swap for its link, or pointing the links at it): from then on the
    /// plugin may have saved into it, so it's never cleared for good.
    in_use: bool,
}

/// A folder holding a marker: a copy a move started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HalfCopy {
    /// The folder.
    pub path: String,
    /// The folder it's a copy of.
    pub from: String,
    /// When the copy started, as the marker says (`2026-11-03T14:03:12.1234567+00:00`).
    pub started: String,
    /// How big it is (0 when only its marker would go).
    pub bytes: u64,
    /// What can be done with it.
    pub state: State,
}

/// What can be done with a half copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Cut off part way: nothing uses it and everything in it is a copy, so it can be cleared.
    CutOff,
    /// It's in use (the move went through), or was renamed since: only its marker is left over, and only that goes.
    MarkerOnly,
    /// It stays, and why.
    Stays(String),
}

/// What clearing did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cleared {
    /// The half copy went, and this much room with it.
    Copy(u64),
    /// Only its marker went: the folder is in use, or was renamed.
    Marker,
}

/// The places a copy is being made or cleared in now, in this app, each with the number of the claim that holds it.
static CLAIMED: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());

/// The number the next claim gets.
static NEXT_CLAIM: AtomicU64 = AtomicU64::new(1);

/// A folder claimed for making or clearing a copy in, until it's dropped. A move claims the copy it makes, from before
/// it clears what an earlier try left there until the copy is in use; clearing claims the half copy it clears. So a
/// copy a move is making is never cleared, and a move never copies into a folder being cleared.
pub(crate) struct Claim {
    number: u64,
}

/// Claims a folder for making or clearing a copy in, or `None` when it's claimed already.
pub(crate) fn claim(folder: &Path) -> Option<Claim> {
    let spots = spots(folder);
    let mut claimed = CLAIMED.lock().unwrap_or_else(PoisonError::into_inner);
    if claimed.iter().any(|(spot, _)| spots.contains(spot)) {
        return None;
    }
    let number = NEXT_CLAIM.fetch_add(1, Ordering::Relaxed);
    claimed.extend(spots.into_iter().map(|spot| (spot, number)));
    Some(Claim { number })
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut claimed = CLAIMED.lock().unwrap_or_else(PoisonError::into_inner);
        claimed.retain(|(_, number)| *number != self.number);
    }
}

/// Whether a copy is being made or cleared in the folder now, by anything but `mine`.
fn busy(folder: &Path, mine: Option<&Claim>) -> bool {
    let spots = spots(folder);
    let claimed = CLAIMED.lock().unwrap_or_else(PoisonError::into_inner);
    claimed.iter().any(|(spot, number)| mine.is_none_or(|m| m.number != *number) && spots.contains(spot))
}

/// What a move says when the copy it would make is being made or cleared already.
pub(crate) fn busy_now(folder: &Path) -> CantMove {
    CantMove::Failed(format!(
        "The app is making or clearing a copy at {} now. Try again once that's done.",
        text(folder)
    ))
}

/// Where a folder is, as written and as it really is (its folder's real place and its name, so one not made yet is
/// known too).
fn spots(folder: &Path) -> Vec<String> {
    let mut spots = vec![key(&text(folder))];
    if let (Some(parent), Some(name)) = (folder.parent(), folder.file_name())
        && let Some(real) = real_path(&text(parent))
    {
        let spot = key(&text(&Path::new(&real).join(name)));
        if !spots.contains(&spot) {
            spots.push(spot);
        }
    }
    spots
}

/// Puts the marker into a copy just made (`tree.to`), written through to the disk so that a power cut straight after
/// can't leave half of it.
pub(crate) fn mark(tree: &Tree) -> io::Result<()> {
    let marker = Marker {
        from: text(tree.from),
        to: text(tree.to),
        settled: text(tree.settled),
        started: csharp_time(chrono::Local::now()),
        in_use: false,
    };
    let path = tree.to.join(MARKER);
    let mut file = OpenOptions::new().write(true).create_new(true).open(&path)?;
    let written = write_marker(&mut file, &marker);
    if written.is_err() {
        drop(file);
        let _ = fs::remove_file(&path);
    }
    written
}

/// Writes a marker into its file, through to the disk.
fn write_marker(file: &mut fs::File, marker: &Marker) -> io::Result<()> {
    let json = serde_json::to_string(marker).map_err(io::Error::other)?;
    file.write_all(format!("{EXPLAINED}\n\n{json}\n").as_bytes())?;
    file.sync_all()
}

/// Just before a copy goes into use (the swap for its link, or pointing the links at it): notes that in its marker,
/// so it's never cleared for good from then on, or, when that can't be written, takes the marker out (a copy without
/// one is never cleared either). `false` when neither could be done: the copy mustn't go into use then.
pub(crate) fn going_into_use(folder: &Path) -> bool {
    note_in_use(folder, true).is_ok() || unmark(folder)
}

/// When a copy didn't go into use after all (its swap didn't happen, and nothing leads to it), just before it's
/// removed: its marker says so again, so a copy that can't all go now (a file in it is open) is cleared by the next
/// try, as one cut off part way is. A marker that can't be written stays as it was.
pub(crate) fn not_in_use(folder: &Path) {
    let _ = note_in_use(folder, false);
}

/// Writes in a copy's marker whether it's going into use (when it doesn't say so already).
fn note_in_use(folder: &Path, in_use: bool) -> io::Result<()> {
    let mut marker = read_marker(folder).ok_or_else(|| io::Error::other("the marker can't be read"))?;
    if marker.in_use == in_use {
        return Ok(());
    }
    marker.in_use = in_use;
    // Written over, not beside: a marker cut off part way can't be read, and a folder with none is never cleared.
    let mut file = OpenOptions::new().write(true).truncate(true).open(folder.join(MARKER))?;
    write_marker(&mut file, &marker)
}

/// The marker in a folder, if it holds one the app wrote.
fn read_marker(folder: &Path) -> Option<Marker> {
    let body = String::from_utf8(marker_bytes(folder)?).ok()?;
    let line = body.lines().rev().map(str::trim).find(|l| l.starts_with('{'))?;
    serde_json::from_str(line).ok()
}

/// What the marker file in a folder holds, if it's a plain file no bigger than a marker the app writes.
fn marker_bytes(folder: &Path) -> Option<Vec<u8>> {
    let path = folder.join(MARKER);
    let meta = fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MARKER_MOST {
        return None;
    }
    fs::read(&path).ok()
}

/// Takes the marker out of a copy that's in use now, trying again for a few seconds while something (a virus checker)
/// has it open. Only ever a plain file of the marker's name. Says whether it's gone.
pub(crate) fn unmark(folder: &Path) -> bool {
    let path = folder.join(MARKER);
    for _ in 0..20 {
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return true,
            Ok(m) if !m.is_file() => return false,
            _ => {}
        }
        match fs::remove_file(&path) {
            Ok(()) => return true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return true,
            Err(_) => thread::sleep(Duration::from_millis(250)),
        }
    }
    false
}

/// The half copy `folder` is, if it holds a marker the app wrote. `moved` is every moved folder. Reads all of it and
/// measures it: call it away from the window's thread.
pub fn find(folder: &Path, moved: &[MovedFolder], stop: &AtomicBool) -> Result<Option<HalfCopy>, Stopped> {
    let Some((marker, state, counted)) = judged(folder, moved, None, stop)? else { return Ok(None) };
    let bytes = match (&state, counted) {
        (State::MarkerOnly, _) => 0,
        (_, Some(bytes)) => bytes,
        _ => size::measure(folder, stop)?.bytes,
    };
    Ok(Some(HalfCopy { path: text(folder), from: marker.from, started: marker.started, bytes, state }))
}

/// The half copies in the places moves go to: the folder each moved folder's copy is in, the folder each moved folder
/// is in (moving back makes its copy there), and `also` (the folders the app remembers moves going to, such as the one
/// picked last). Each folder in them holding a marker, once; not one a move is making now. Measures them: call it away
/// from the window's thread.
pub fn half_copies(moved: &[MovedFolder], also: &[String], stop: &AtomicBool) -> Result<Vec<HalfCopy>, Stopped> {
    let mut places: Vec<String> = also.to_vec();
    places.extend(moved.iter().flat_map(|m| [parent_of(&m.to), parent_of(&m.from)]).flatten());
    let mut found: Vec<HalfCopy> = Vec::new();
    for entry in in_places(places, stop)? {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) || not_there(&path.join(MARKER)) || busy(&path, None) {
            continue;
        }
        if let Some(half) = find(&path, moved, stop)?
            && !found.iter().any(|h| same_place(&h.path, &half.path))
        {
            found.push(half);
        }
    }
    Ok(found)
}

/// Clears a half copy: looked at again first, then removed for good when it was cut off, or only its marker taken out
/// when the folder is in use or was renamed. Nothing else in this app makes or clears a copy there meanwhile. The
/// marker goes last, so a clear that stops part way (you press Stop, or a file in it is open) is tried again next time.
/// `Err` says why it stays. Call it away from the window's thread.
pub fn clear(folder: &Path, moved: &[MovedFolder], stop: &AtomicBool) -> Result<Cleared, String> {
    let Some(mine) = claim(folder) else { return Err(BUSY.to_owned()) };
    let stopped = || "Stopped before it was all checked, so it stays.".to_owned();
    let Some((_, state, counted)) = judged(folder, moved, Some(&mine), stop).map_err(|Stopped| stopped())? else {
        return Err(format!("{} holds no marker, so it isn't a half copy the app made: it stays.", text(folder)));
    };
    match state {
        State::Stays(why) => Err(why),
        State::MarkerOnly if unmark(folder) => Ok(Cleared::Marker),
        State::MarkerOnly => {
            Err(format!("The marker in {} couldn't be taken out: it's tried again next time.", text(folder)))
        }
        State::CutOff => match remove_half(folder, stop) {
            Ok(()) => Ok(Cleared::Copy(counted.unwrap_or(0))),
            Err(NotRemoved::Stopped) => {
                Err("Stopped part way: what's left of it keeps its marker, and is cleared the next time.".to_owned())
            }
            Err(NotRemoved::Failed(why)) => Err(why),
        },
    }
}

/// Before a move copies into `copy` (a new folder of that name, claimed by `mine`): clears the half copy an earlier
/// try at the same copy left there, cut off part way (its marker says it copies `from`), so it isn't in the way. A
/// marker left in a copy that's in use, or in a folder renamed since, is taken out. Anything else there is left as it
/// is, for the move's own checks to name; one that's this copy's but stays says why.
pub(crate) fn clear_own(
    copy: &Path,
    from: &str,
    moved: &[MovedFolder],
    mine: &Claim,
    stop: &AtomicBool,
) -> Result<(), CantMove> {
    match own(copy, from, moved, Some(mine), stop)? {
        Some((State::CutOff, _)) => remove_half(copy, stop).map_err(|not| match not {
            NotRemoved::Stopped => CantMove::Stopped,
            NotRemoved::Failed(why) => CantMove::HalfCopy { path: text(copy), why },
        }),
        Some((State::MarkerOnly, _)) => {
            unmark(copy);
            Ok(())
        }
        Some((State::Stays(why), _)) => Err(CantMove::HalfCopy { path: text(copy), why }),
        None => Ok(()),
    }
}

/// How much room the half copy an earlier try at copying `from` into `copy` left takes, when it was cut off and the
/// move clears it first. `mine` is the claim the move holds on `copy`, if any. `Err` when it's this copy's but stays.
pub(crate) fn own_room(
    copy: &Path,
    from: &str,
    moved: &[MovedFolder],
    mine: Option<&Claim>,
    stop: &AtomicBool,
) -> Result<Option<u64>, CantMove> {
    match own(copy, from, moved, mine, stop)? {
        Some((State::CutOff, bytes)) => Ok(bytes),
        Some((State::Stays(why), _)) => Err(CantMove::HalfCopy { path: text(copy), why }),
        _ => Ok(None),
    }
}

/// What can be done with the half copy at `copy`, if it's one an earlier try at copying `from` there left, and how big
/// it is when that's been counted. `mine` is the claim the caller holds on it, if any.
fn own(
    copy: &Path,
    from: &str,
    moved: &[MovedFolder],
    mine: Option<&Claim>,
    stop: &AtomicBool,
) -> Result<Option<(State, Option<u64>)>, CantMove> {
    // A link, or a file, is never a half copy: it's left for the move's own checks to name.
    if !real_folder(&text(copy)) {
        return Ok(None);
    }
    match read_marker(copy) {
        Some(marker) if same_place(&marker.from, from) => {
            if busy(copy, mine) {
                return Err(busy_now(copy));
            }
            let judged = judged(copy, moved, mine, stop).map_err(|Stopped| CantMove::Stopped)?;
            Ok(judged.map(|(_, state, bytes)| (state, bytes)))
        }
        _ => Ok(None),
    }
}

/// Removes a half copy that was found cut off, once its marker is seen to be there still and not yet saying the copy
/// is going into use: one whose marker has gone, or changed, may be in use now. Stops when you press Stop.
fn remove_half(folder: &Path, stop: &AtomicBool) -> Result<(), NotRemoved> {
    match read_marker(folder) {
        Some(marker) if !marker.in_use => remove_marked(folder, stop),
        _ => Err(NotRemoved::Failed(format!(
            "The marker in {} changed as it was cleared, so the rest stays.",
            text(folder)
        ))),
    }
}

/// Whether two paths name one place: the same as written, or where they really are.
fn same_place(a: &str, b: &str) -> bool {
    key(a) == key(b) || matches!((real_path(a), real_path(b)), (Some(x), Some(y)) if key(&x) == key(&y))
}

/// What [`judged`] finds: the marker, what can be done with the folder, and how big it is when that's been counted.
type Judged = (Marker, State, Option<u64>);

/// A folder's marker, what can be done with the folder, and how big it is when that's been counted; `None` when it
/// isn't a real folder holding a marker. `mine` is the claim the caller holds on it, if any.
fn judged(
    folder: &Path,
    moved: &[MovedFolder],
    mine: Option<&Claim>,
    stop: &AtomicBool,
) -> Result<Option<Judged>, Stopped> {
    if !real_folder(&text(folder)) {
        return Ok(None);
    }
    let Some(marker) = read_marker(folder) else { return Ok(None) };
    let (state, bytes) = judge(folder, &marker, moved, mine, stop)?;
    Ok(Some((marker, state, bytes)))
}

/// What can be done with a folder holding a marker, from the checks in this module's notes, in order; and how big it
/// is when that's been counted (everything in it is looked at for the last check).
fn judge(
    folder: &Path,
    marker: &Marker,
    moved: &[MovedFolder],
    mine: Option<&Claim>,
    stop: &AtomicBool,
) -> Result<(State, Option<u64>), Stopped> {
    let stays = |why: String| Ok((State::Stays(why), None));
    if busy(folder, mine) {
        return stays(BUSY.to_owned());
    }
    let Some(here) = real_path(&text(folder)) else { return stays(gone(folder)) };
    // Renamed since it was marked: into place (moving back), or by you. It isn't where the copy was being made.
    if !in_place(folder, marker, &here) {
        return Ok((State::MarkerOnly, None));
    }
    let from = marker.from.as_str();
    // What it copies leads here, or a moved folder uses it: it's the copy in use.
    if real_path(from).is_some_and(|f| key(&f) == key(&here)) || used(folder, &here, moved) {
        return Ok((State::MarkerOnly, None));
    }
    if marker.in_use {
        return stays(went_into_use(folder, marker));
    }
    if !is_dir(from) {
        return stays(format!(
            "{from} isn't there now, so this copy may be the only one of its files. Look in it before deleting it \
             yourself."
        ));
    }
    let settled = marker.settled.as_str();
    // Moving back: until the swap, the link is in the folder's place, leading to what's copied.
    if key(settled) != key(&marker.to) && !(is_link(Path::new(settled)) && leads_to(settled, Path::new(from))) {
        return stays(format!(
            "{settled} isn't the link to {from} now, so this copy may be in use. Look at both first."
        ));
    }
    let (only, bytes) = only_here(folder, Path::new(from), stop)?;
    if !only.is_empty() {
        let why = format!(
            "It holds what isn't in {from} now, or is newer than there: {}. Look at it before deleting it yourself.",
            listed(&only)
        );
        return Ok((State::Stays(why), Some(bytes)));
    }
    Ok((State::CutOff, Some(bytes)))
}

/// Whether the folder is where its marker says the copy was made, by what's at that place now: a real folder, when
/// it's this one as it really is; nothing at all (the drive it's on has another letter now, maybe another drive's old
/// one, or a Mac gave it another name), when this one has the same name. A link there (left at the name of a half copy
/// renamed since, leading to it), a file, or what can't be looked at isn't this folder.
fn in_place(folder: &Path, marker: &Marker, here: &str) -> bool {
    match fs::symlink_metadata(&marker.to) {
        Ok(meta) if meta.is_dir() => real_path(&marker.to).is_some_and(|to| key(&to) == key(here)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            folder.file_name().is_some_and(|name| key(&name.to_string_lossy()) == key(file_name(&marker.to)))
        }
        _ => false,
    }
}

/// Why a copy that started going into use, and isn't in use now, stays: the plugin may have saved into it. When the
/// swap itself was cut off, what to rename back.
fn went_into_use(folder: &Path, marker: &Marker) -> String {
    let from = marker.from.as_str();
    let settled = marker.settled.as_str();
    if key(settled) != key(&marker.to) {
        // Moving back: the link goes, then the copy takes the folder's name.
        if not_there(Path::new(settled)) {
            return format!(
                "Moving back was cut off as it swapped: the link at {settled} is gone. This copy is whole: rename \
                 \"{}\" to \"{}\", and the plugin finds its files again.",
                file_name(&text(folder)),
                file_name(settled)
            );
        }
        return format!(
            "Moving back was cut off just as this copy went into use, so the plugin may have saved into it. Look at it \
             and {settled} first, then delete the one you don't need yourself."
        );
    }
    let aside = format!("{from}{OLD_SUFFIX}");
    if real_folder(&aside) && not_there(Path::new(from)) {
        return format!(
            "The move was cut off as it swapped the folder for its link: {from} is called \"{}\" now, and this copy is \
             whole. Rename it back to \"{}\", then delete this copy yourself and move the folder again.",
            file_name(&aside),
            file_name(from.trim_end_matches(['/', '\\']))
        );
    }
    format!(
        "The move was cut off just as this copy went into use, so the plugin may have saved into it. Look at it and \
         {from} first, then delete the one you don't need yourself."
    )
}

fn gone(folder: &Path) -> String {
    format!("{} can't be looked at now, so it stays.", text(folder))
}

/// Whether a moved folder uses the folder (`here` is where it really is): its link leads to it or into it, or its
/// copy is the folder or inside it.
fn used(folder: &Path, here: &str, moved: &[MovedFolder]) -> bool {
    let path = text(folder);
    moved.iter().any(|m| {
        let link_in = is_link(Path::new(&m.from)) && real_path(&m.from).is_some_and(|r| is_inside(&r, here));
        let copy_in = is_inside(&m.to, &path) || real_path(&m.to).is_some_and(|r| is_inside(&r, here));
        link_in || copy_in
    })
}

/// What's in the copy that isn't a copy of what's in `from` now, as paths inside it, and how big the copy is. Not a
/// copy: something without its twin in `from` at the same place, of the same kind (a file for a file, or for a file
/// link: Windows may not let one be made again, so its file is copied), and a file newer than its twin. One file
/// newer but no bigger is let through: the one the copy was writing when it stopped (not whole yet, or not given its
/// time yet). Left out: the marker at the top, a file the copy was writing under its temporary name (when its own file
/// is in `from`), and the files Explorer and Finder make by themselves. A folder in it that can't be read, or is deeper
/// than walks go, is named too: what's in it can't be told. The size is all of it, what's named too (a folder without
/// a twin is walked only to be counted), as far as it can be read and as deep as walks go: one walk for both. Walked a
/// level at a time on every core of the pool it's called in.
fn only_here(copy: &Path, from: &Path, stop: &AtomicBool) -> Result<(Vec<String>, u64), Stopped> {
    let mut only = Vec::new();
    let mut writing = Vec::new();
    let mut bytes: u64 = 0;
    let mut level = vec![(copy.to_path_buf(), Some(from.to_path_buf()))];
    for depth in 0..DEEPEST {
        if level.is_empty() {
            break;
        }
        if stop.load(Ordering::Relaxed) {
            return Err(Stopped);
        }
        let read: Vec<Compared> =
            level.par_iter().map(|(dir, twin)| compare(copy, dir, twin.as_deref(), depth == 0, stop)).collect();
        level = Vec::new();
        for compared in read {
            only.extend(compared.only);
            writing.extend(compared.writing);
            bytes = bytes.saturating_add(compared.bytes);
            level.extend(compared.folders);
        }
    }
    if stop.load(Ordering::Relaxed) {
        return Err(Stopped);
    }
    // Deeper than walks go: what's in them can't be told (one without a twin is named already).
    only.extend(level.iter().filter(|(_, twin)| twin.is_some()).map(|(dir, _)| inside(copy, dir)));
    // One file newer than its twin but no bigger is the one the copy was writing when it stopped; two or more aren't.
    if writing.len() > 1 {
        only.append(&mut writing);
    }
    only.sort();
    Ok((only, bytes))
}

/// What one folder of a half copy holds, beside its twin in the folder it copies from, as [`only_here`] looks at it.
#[derive(Default)]
struct Compared {
    /// What isn't a copy (paths inside the half copy).
    only: Vec<String>,
    /// Files newer than their twins but no bigger: the copy may have been writing one of them when it stopped.
    writing: Vec<String>,
    /// The room its files take.
    bytes: u64,
    /// The folders in it to look in next, each with its twin; one without a twin (named already) is only counted.
    folders: Vec<(PathBuf, Option<PathBuf>)>,
}

/// Compares one folder of a half copy (`dir`, the copy's top when `top`) with its twin, or, with none (it's named
/// already, or inside what is), only counts it. Looks at Stop before it starts and every 256 entries; one stopped part
/// way says no more.
fn compare(copy: &Path, dir: &Path, twin_dir: Option<&Path>, top: bool, stop: &AtomicBool) -> Compared {
    let mut compared = Compared::default();
    if stop.load(Ordering::Relaxed) {
        return compared;
    }
    // What can't be read inside what's named already needn't be named again.
    let cant_tell = |compared: &mut Compared, path: &Path| {
        if twin_dir.is_some() {
            compared.only.push(inside(copy, path));
        }
    };
    let Ok(entries) = fs::read_dir(dir) else {
        cant_tell(&mut compared, dir);
        return compared;
    };
    for (n, entry) in entries.enumerate() {
        if n % 256 == 255 && stop.load(Ordering::Relaxed) {
            break;
        }
        let Ok(entry) = entry else {
            cant_tell(&mut compared, dir);
            continue;
        };
        let name = entry.file_name();
        let path = entry.path();
        // As it is, not where a link leads.
        let Ok(meta) = entry.metadata() else {
            cant_tell(&mut compared, &path);
            continue;
        };
        let kind = meta.file_type();
        if kind.is_file() {
            compared.bytes = compared.bytes.saturating_add(meta.len());
        }
        let Some(twin_dir) = twin_dir else {
            if kind.is_dir() {
                compared.folders.push((path, None));
            }
            continue;
        };
        if kind.is_file() {
            if (top && name == OsStr::new(MARKER)) || made_by_the_system(&name.to_string_lossy()) {
                continue;
            }
            // A file the copy was writing over an older one when it stopped, named after a file still in `from`.
            let own_part = name.to_string_lossy().strip_suffix(PART_SUFFIX).map(|own| twin_dir.join(own));
            if own_part.is_some_and(|own| fs::symlink_metadata(own).is_ok_and(|m| m.is_file())) {
                continue;
            }
        }
        let twin = twin_dir.join(&name);
        let Ok(twin_meta) = fs::symlink_metadata(&twin) else {
            compared.only.push(inside(copy, &path));
            if kind.is_dir() {
                compared.folders.push((path, None));
            }
            continue;
        };
        let twin_kind = twin_meta.file_type();
        if kind.is_symlink() {
            if !twin_kind.is_symlink() {
                compared.only.push(inside(copy, &path));
            }
        } else if kind.is_dir() {
            if twin_kind.is_dir() {
                compared.folders.push((path, Some(twin)));
            } else {
                compared.only.push(inside(copy, &path));
                compared.folders.push((path, None));
            }
        } else {
            // A file's twin is a file, or a link to one, as where it leads.
            let twin_file = if twin_kind.is_symlink() { fs::metadata(&twin).ok() } else { Some(twin_meta) };
            match twin_file.filter(fs::Metadata::is_file) {
                Some(t) if same_file(&meta, &t) || newer(&t, &meta) => {}
                Some(t) if meta.len() <= t.len() => compared.writing.push(inside(copy, &path)),
                _ => compared.only.push(inside(copy, &path)),
            }
        }
    }
    compared
}

/// A path in a half copy as it's named to you: from the copy's top.
fn inside(copy: &Path, path: &Path) -> String {
    text(path.strip_prefix(copy).unwrap_or(path))
}

/// Why a copy the move made wasn't all removed.
#[derive(Debug)]
pub(crate) enum NotRemoved {
    /// You pressed Stop: what's left keeps its marker, so it's cleared the next time.
    Stopped,
    /// Something in it couldn't be removed: what, and why.
    Failed(String),
}

/// Removes a copy the move made, for good, keeping its marker (if it has one) to the last: everything else in it goes
/// first, then the marker, then the folder ([`finish`]). So a copy that can't all go (a file in it is open), or whose
/// clearing you stop (`stop`, looked at between the things it removes), keeps its marker, and is cleared when it's
/// tried again. Never a link, never through one: a link inside goes as the link.
pub(crate) fn remove_marked(folder: &Path, stop: &AtomicBool) -> Result<(), NotRemoved> {
    if is_link(folder) || !real_folder(&text(folder)) {
        return Ok(());
    }
    let entries =
        fs::read_dir(folder).and_then(Iterator::collect::<io::Result<Vec<_>>>).map_err(|e| couldnt(folder, &e))?;
    for entry in entries {
        if stop.load(Ordering::Relaxed) {
            return Err(NotRemoved::Stopped);
        }
        if entry.file_name() == OsStr::new(MARKER) {
            continue;
        }
        remove_entry(&entry.path(), stop)?;
    }
    finish(folder)
}

/// How many times [`finish`] looks at the folder, and tries it, a tenth of a second apart.
const LAST_TRIES: usize = 5;

/// Takes the marker out of a copy that's empty but for it, then the folder. Something put in it meanwhile keeps it, and
/// its marker, but the files Explorer and Finder write by themselves into a folder open in a window: those go too. A
/// virus checker or the indexer can hold a file that's been removed for a moment, so it's looked at, and the folder
/// tried, a few times; if it still won't go, the marker goes back as it was, so the folder is still known for the half
/// copy it is, and cleared the next time.
fn finish(folder: &Path) -> Result<(), NotRemoved> {
    let mut left = put_in(folder);
    for _ in 1..LAST_TRIES {
        if left.is_none() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
        left = put_in(folder);
    }
    if let Some(name) = left {
        return Err(NotRemoved::Failed(format!(
            "{name} is still in {} (something put it there, or has it open), so the folder stays: it's looked at \
             again next time.",
            text(folder)
        )));
    }
    let marker = marker_bytes(folder);
    if !unmark(folder) {
        return Err(NotRemoved::Failed(format!(
            "The marker in {} couldn't be removed: it's tried again next time.",
            text(folder)
        )));
    }
    let mut tries = 1;
    loop {
        remove_system_files(folder);
        match fs::remove_dir(folder) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) if tries == LAST_TRIES => {
                if let Some(marker) = &marker {
                    put_marker_back(folder, marker);
                }
                return Err(couldnt(folder, &e));
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(100));
                tries += 1;
            }
        }
    }
}

/// The name of something in a copy being cleared that isn't its marker or a file Explorer or Finder made, if any.
fn put_in(folder: &Path) -> Option<String> {
    fs::read_dir(folder).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name();
        let theirs = name == OsStr::new(MARKER) || system_file(&entry);
        (!theirs).then(|| name.to_string_lossy().into_owned())
    })
}

/// Whether a folder's entry is a plain file Explorer or Finder made by themselves.
fn system_file(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|kind| kind.is_file()) && made_by_the_system(&entry.file_name().to_string_lossy())
}

/// Removes the files Explorer and Finder make by themselves from a folder.
fn remove_system_files(folder: &Path) {
    for entry in fs::read_dir(folder).into_iter().flatten().flatten().filter(system_file) {
        let path = entry.path();
        set_read_only(&path, false);
        let _ = fs::remove_file(path);
    }
}

/// Puts a marker taken out back as it was, written through to the disk. Only where nothing has its name.
fn put_marker_back(folder: &Path, marker: &[u8]) {
    if let Ok(mut file) = OpenOptions::new().write(true).create_new(true).open(folder.join(MARKER)) {
        let _ = file.write_all(marker).and_then(|()| file.sync_all());
    }
}

/// What's said when something in a copy the move made can't be removed.
fn couldnt(path: &Path, e: &io::Error) -> NotRemoved {
    NotRemoved::Failed(format!(
        "{} couldn't be removed ({e}): the rest of the half copy is cleared when it's tried again.",
        text(path)
    ))
}

/// Removes one thing in a copy the move made: a link as the link, a folder with all that's in it (links in it as
/// links, never followed), a file even when it's read-only (Windows won't remove those otherwise, and on a drive
/// without newer Windows' way of deleting, such as exFAT, not even inside a folder being removed). Looks at `stop`
/// between the things in it.
fn remove_entry(path: &Path, stop: &AtomicBool) -> Result<(), NotRemoved> {
    let mut folders: Vec<(PathBuf, bool)> = Vec::new();
    let kind = fs::symlink_metadata(path).map_err(|e| couldnt(path, &e))?.file_type();
    remove_one(path, kind, &mut folders).map_err(|e| couldnt(path, &e))?;
    // Each folder is emptied, then removed once what's in it has gone.
    while let Some((dir, emptied)) = folders.pop() {
        if stop.load(Ordering::Relaxed) {
            return Err(NotRemoved::Stopped);
        }
        if emptied {
            fs::remove_dir(&dir).map_err(|e| couldnt(&dir, &e))?;
            continue;
        }
        // Looked at again just before it's read: one swapped for a link meanwhile goes as the link, and what it leads
        // to stays.
        let kind = fs::symlink_metadata(&dir).map_err(|e| couldnt(&dir, &e))?.file_type();
        if !kind.is_dir() {
            remove_one(&dir, kind, &mut folders).map_err(|e| couldnt(&dir, &e))?;
            continue;
        }
        folders.push((dir.clone(), true));
        let entries =
            fs::read_dir(&dir).and_then(Iterator::collect::<io::Result<Vec<_>>>).map_err(|e| couldnt(&dir, &e))?;
        for entry in entries {
            if stop.load(Ordering::Relaxed) {
                return Err(NotRemoved::Stopped);
            }
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| couldnt(&path, &e))?;
            remove_one(&path, kind, &mut folders).map_err(|e| couldnt(&path, &e))?;
        }
    }
    Ok(())
}

/// Removes a link or a file at once; a folder goes on `folders`, to be emptied first.
fn remove_one(path: &Path, kind: fs::FileType, folders: &mut Vec<(PathBuf, bool)>) -> io::Result<()> {
    if kind.is_symlink() {
        // A junction or a folder link goes as a folder on Windows, a file link as a file.
        remove_link(path).or_else(|_| fs::remove_file(path))
    } else if kind.is_dir() {
        folders.push((path.to_path_buf(), false));
        Ok(())
    } else {
        set_read_only(path, false);
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::AtomicBool;

    use super::{MARKER, NotRemoved, finish, remove_entry, remove_marked};

    #[test]
    fn the_folder_goes_last_with_a_file_finder_wrote_but_not_with_one_of_yours() {
        let dir = std::env::temp_dir().join("mo-space-unit-clear-finish");
        let _ = fs::remove_dir_all(&dir);
        let half = dir.join("Presets");
        fs::create_dir_all(&half).unwrap();
        fs::write(half.join(MARKER), "a marker").unwrap();
        // Finder wrote its file into the emptied folder, open in a window: it goes too.
        fs::write(half.join(".DS_Store"), "finder").unwrap();
        assert!(finish(&half).is_ok());
        assert!(!half.exists());

        // Something else was put in it meanwhile: the folder stays, with its marker as it was.
        fs::create_dir_all(&half).unwrap();
        fs::write(half.join(MARKER), "a marker").unwrap();
        fs::write(half.join("keep.txt"), "yours").unwrap();
        assert!(matches!(finish(&half), Err(NotRemoved::Failed(_))));
        assert_eq!(fs::read_to_string(half.join(MARKER)).unwrap(), "a marker");
        assert_eq!(fs::read_to_string(half.join("keep.txt")).unwrap(), "yours");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clearing_stops_when_you_press_stop_and_the_marker_stays() {
        let dir = std::env::temp_dir().join("mo-space-unit-clear-stop");
        let _ = fs::remove_dir_all(&dir);
        let half = dir.join("Presets");
        fs::create_dir_all(half.join("Sounds")).unwrap();
        fs::write(half.join(MARKER), "a marker").unwrap();
        fs::write(half.join("Sounds").join("a.db"), "a").unwrap();
        fs::write(half.join("Patch.prt"), "p").unwrap();
        let stopped = AtomicBool::new(true);

        // Stopped before it starts on the next thing, inside a folder too.
        assert!(matches!(remove_marked(&half, &stopped), Err(NotRemoved::Stopped)));
        assert!(matches!(remove_entry(&half.join("Sounds"), &stopped), Err(NotRemoved::Stopped)));
        assert!(half.join(MARKER).exists() && half.join("Sounds").join("a.db").exists());
        // Not stopped, all of it goes.
        assert!(remove_marked(&half, &AtomicBool::new(false)).is_ok());
        assert!(!half.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
