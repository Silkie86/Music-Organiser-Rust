//! Half copies: what a move was copying when the app or the computer stopped (a crash, a power cut, the app closed
//! part way). Every copy a move makes as a new folder (moving a folder to another drive, moving it back once its old
//! copy is gone, moving the folder the moved folders live in) gets a marker the moment it's made: a small text file
//! inside it, [`MARKER`], saying what it's a copy of. The marker comes out once the copy is in use, so a copy that
//! still holds one was cut off part way (Solomon, 3 Nov 2026: yes to "should the move leave a marker so the app can
//! clear it?"). The app clears it then: the next try at the same move does that first, and the Space page finds the
//! others in the places moves go to ([`half_copies`]) and clears them ([`clear`]).
//!
//! Cleared means removed for good, as a move removes its copy when you press Stop: a half copy only ever holds copies
//! the move made. And only while all of this holds, looked at again just before:
//!
//! - the marker is a plain file in the folder it names, and the folder is a real folder, not a link;
//! - the folder it copies from is there, and isn't a link (the swap didn't happen: it makes one);
//! - no moved folder's link leads to it or into it, and no moved folder's copy is it or inside it;
//! - moving back, the link is still in the folder's place, leading to what was being copied (the swap didn't start);
//!   moving, there's no "… (old copy)" beside the folder it copies from (the swap makes one);
//! - everything in it has its twin in the folder it copies from, at the same place and of the same kind (a file for a
//!   file, a folder for a folder, a link for a link), leaving out the marker, a file the copy was writing under its
//!   temporary name, and the files Explorer and Finder make by themselves.
//!
//! Otherwise it stays, and says why. A marker in a copy that's in use (the app stopped between the swap and taking the
//! marker out), or in a folder renamed since, is the only thing that goes then: the folder is left as it is.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use mo_formats::paths::file_name;
use mo_library::scan::is_inside;
use serde::{Deserialize, Serialize};

use crate::folder_move::{
    CantMove, MovedFolder, OLD_SUFFIX, PART_SUFFIX, Tree, csharp_time, leads_to, listed, remove_link, set_read_only,
};
use crate::plugin_clean::made_by_the_system;
use crate::size::{self, DEEPEST, Stopped, is_link};
use crate::{is_dir, key, not_there, real_folder, real_path, text};

/// The marker's name, at the top of a copy a move is making.
pub const MARKER: &str = "Music Organiser - copy not finished.txt";

/// What the marker says first, for anyone who opens it.
const EXPLAINED: &str = "Music Organiser is copying a folder into this one. If this file is still here once the app \
                         has finished, the copy was cut off part way (the app or the computer stopped), so this folder \
                         is a half copy: the app clears it the next time it moves the folder, or from the Space page. \
                         The folder it copies from wasn't changed.";

/// The biggest marker read: anything bigger isn't one the app wrote.
const MARKER_MOST: u64 = 64 << 10;

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

/// Puts the marker into a copy just made (`tree.to`), written through to the disk so that a power cut straight after
/// can't leave half of it.
pub(crate) fn mark(tree: &Tree) -> io::Result<()> {
    let marker = Marker {
        from: text(tree.from),
        to: text(tree.to),
        settled: text(tree.settled),
        started: csharp_time(chrono::Local::now()),
    };
    let json = serde_json::to_string(&marker).map_err(io::Error::other)?;
    let path = tree.to.join(MARKER);
    let mut file = OpenOptions::new().write(true).create_new(true).open(&path)?;
    let written = file.write_all(format!("{EXPLAINED}\n\n{json}\n").as_bytes()).and_then(|()| file.sync_all());
    if written.is_err() {
        drop(file);
        let _ = fs::remove_file(&path);
    }
    written
}

/// The marker in a folder, if it holds one the app wrote.
fn read_marker(folder: &Path) -> Option<Marker> {
    let path = folder.join(MARKER);
    let meta = fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MARKER_MOST {
        return None;
    }
    let body = fs::read_to_string(&path).ok()?;
    let line = body.lines().rev().map(str::trim).find(|l| l.starts_with('{'))?;
    serde_json::from_str(line).ok()
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
    let Some((marker, state)) = judged(folder, moved, stop)? else { return Ok(None) };
    let bytes = if state == State::MarkerOnly { 0 } else { size::measure(folder, stop)?.bytes };
    Ok(Some(HalfCopy { path: text(folder), from: marker.from, started: marker.started, bytes, state }))
}

/// The half copies in the places moves go to: the folder each moved folder's copy is in, the folder each moved folder
/// is in (moving back makes its copy there), and `also` (the folders the app remembers moves going to, such as the one
/// picked last). Each folder in them holding a marker, once. Measures them: call it away from the window's thread.
pub fn half_copies(moved: &[MovedFolder], also: &[String], stop: &AtomicBool) -> Result<Vec<HalfCopy>, Stopped> {
    let parent = |p: &str| Path::new(p.trim_end_matches(['/', '\\'])).parent().map(text);
    let mut places: Vec<String> = also.to_vec();
    places.extend(moved.iter().flat_map(|m| [parent(&m.to), parent(&m.from)]).flatten());
    let mut looked: Vec<String> = Vec::new();
    let mut found: Vec<HalfCopy> = Vec::new();
    for place in places {
        if place.is_empty() {
            continue;
        }
        let where_really = real_path(&place).map_or_else(|| key(&place), |r| key(&r));
        if looked.contains(&where_really) {
            continue;
        }
        looked.push(where_really);
        let Ok(entries) = fs::read_dir(&place) else { continue };
        for entry in entries.flatten() {
            if stop.load(Ordering::Relaxed) {
                return Err(Stopped);
            }
            let path = entry.path();
            if !entry.file_type().is_ok_and(|t| t.is_dir()) || not_there(&path.join(MARKER)) {
                continue;
            }
            if let Some(half) = find(&path, moved, stop)?
                && !found.iter().any(|h| same_place(&h.path, &half.path))
            {
                found.push(half);
            }
        }
    }
    Ok(found)
}

/// Clears a half copy: looked at again first ([`find`]), then removed for good when it was cut off, or only its marker
/// taken out when the folder is in use or was renamed. The marker goes last, so a clear that stops part way (a file
/// in it is open) is tried again next time. `Err` says why it stays. Call it away from the window's thread.
pub fn clear(folder: &Path, moved: &[MovedFolder], stop: &AtomicBool) -> Result<Cleared, String> {
    let stopped = || "Stopped before it was all checked, so it stays.".to_owned();
    let Some((_, state)) = judged(folder, moved, stop).map_err(|Stopped| stopped())? else {
        return Err(format!("{} holds no marker, so it isn't a half copy the app made: it stays.", text(folder)));
    };
    match state {
        State::Stays(why) => Err(why),
        State::MarkerOnly if unmark(folder) => Ok(Cleared::Marker),
        State::MarkerOnly => {
            Err(format!("The marker in {} couldn't be taken out: it's tried again next time.", text(folder)))
        }
        State::CutOff => {
            let bytes = size::measure(folder, stop).map_err(|Stopped| stopped())?.bytes;
            remove_marked(folder).map(|()| Cleared::Copy(bytes))
        }
    }
}

/// Before a move copies into `copy` (a new folder of that name): clears the half copy an earlier try at the same copy
/// left there, cut off part way (its marker says it copies `from`), so it isn't in the way. A marker left in a copy
/// that's in use, or in a folder renamed since, is taken out. Anything else there is left as it is, for the move's own
/// checks to name; one that's this copy's but stays says why.
pub(crate) fn clear_own(copy: &Path, from: &str, moved: &[MovedFolder], stop: &AtomicBool) -> Result<(), CantMove> {
    match own(copy, from, moved, stop)? {
        Some(State::CutOff) => remove_marked(copy).map_err(|why| CantMove::HalfCopy { path: text(copy), why }),
        Some(State::MarkerOnly) => {
            unmark(copy);
            Ok(())
        }
        Some(State::Stays(why)) => Err(CantMove::HalfCopy { path: text(copy), why }),
        None => Ok(()),
    }
}

/// How much room the half copy an earlier try at copying `from` into `copy` left takes, when it was cut off and the
/// move clears it first. `Err` when it's this copy's but stays.
pub(crate) fn own_room(
    copy: &Path,
    from: &str,
    moved: &[MovedFolder],
    stop: &AtomicBool,
) -> Result<Option<u64>, CantMove> {
    match own(copy, from, moved, stop)? {
        Some(State::CutOff) => size::measure(copy, stop).map(|m| Some(m.bytes)).map_err(|Stopped| CantMove::Stopped),
        Some(State::Stays(why)) => Err(CantMove::HalfCopy { path: text(copy), why }),
        _ => Ok(None),
    }
}

/// What can be done with the half copy at `copy`, if it's one an earlier try at copying `from` there left.
fn own(copy: &Path, from: &str, moved: &[MovedFolder], stop: &AtomicBool) -> Result<Option<State>, CantMove> {
    match read_marker(copy) {
        Some(marker) if same_place(&marker.from, from) => {
            judged(copy, moved, stop).map(|judged| judged.map(|(_, state)| state)).map_err(|Stopped| CantMove::Stopped)
        }
        _ => Ok(None),
    }
}

/// Whether two paths name one place: the same as written, or where they really are.
fn same_place(a: &str, b: &str) -> bool {
    key(a) == key(b) || matches!((real_path(a), real_path(b)), (Some(x), Some(y)) if key(&x) == key(&y))
}

/// A folder's marker and what can be done with the folder, or `None` when it isn't a real folder holding a marker.
fn judged(folder: &Path, moved: &[MovedFolder], stop: &AtomicBool) -> Result<Option<(Marker, State)>, Stopped> {
    if !real_folder(&text(folder)) {
        return Ok(None);
    }
    let Some(marker) = read_marker(folder) else { return Ok(None) };
    let state = judge(folder, &marker, moved, stop)?;
    Ok(Some((marker, state)))
}

/// What can be done with a folder holding a marker: the checks in this module's notes, in order.
fn judge(folder: &Path, marker: &Marker, moved: &[MovedFolder], stop: &AtomicBool) -> Result<State, Stopped> {
    let Some(here) = real_path(&text(folder)) else { return Ok(State::Stays(gone(folder))) };
    // Renamed since it was marked: into place (moving back), or by you. It isn't where the copy was being made.
    if !real_path(&marker.to).is_some_and(|to| key(&to) == key(&here)) {
        return Ok(State::MarkerOnly);
    }
    let from = marker.from.as_str();
    // What it copies leads here, or is a link at all (it was a real folder while it was copied): the swap went
    // through, so this is, or was, the copy in use.
    if is_link(Path::new(from)) || real_path(from).is_some_and(|f| key(&f) == key(&here)) || used(folder, &here, moved)
    {
        return Ok(State::MarkerOnly);
    }
    let name = file_name(from.trim_end_matches(['/', '\\']));
    if !is_dir(from) {
        let aside = format!("{from}{OLD_SUFFIX}");
        if key(&marker.settled) == key(&marker.to) && real_folder(&aside) && not_there(Path::new(from)) {
            return Ok(State::Stays(format!(
                "The move was cut off as it swapped the folder for its link: {from} is called \"{}\" now. Rename it \
                 back to \"{name}\" (nothing else changed), then try again.",
                file_name(&aside)
            )));
        }
        return Ok(State::Stays(format!(
            "{from} isn't there now, so this copy may be the only one of its files. Look in it before deleting it \
             yourself."
        )));
    }
    let settled = marker.settled.as_str();
    if key(settled) != key(&marker.to) {
        // Moving back: until the swap, the link is in the folder's place, leading to what's copied.
        if !(is_link(Path::new(settled)) && leads_to(settled, Path::new(from))) {
            let why = if not_there(Path::new(settled)) {
                format!(
                    "Moving back was cut off as it swapped: the link at {settled} is gone. This copy is whole: rename \
                     \"{}\" to \"{}\", and the plugin finds its files again.",
                    file_name(&text(folder)),
                    file_name(settled)
                )
            } else {
                format!("{settled} isn't the link to {from} now, so this copy may be in use. Look at both first.")
            };
            return Ok(State::Stays(why));
        }
    } else if real_folder(&format!("{from}{OLD_SUFFIX}")) {
        // Moving: the swap names the folder "… (old copy)", so with one beside it this copy may be the one in use.
        return Ok(State::Stays(format!(
            "There's a \"{name}{OLD_SUFFIX}\" beside {from}, so this copy may be the one the plugin uses. Look at both \
             first."
        )));
    }
    let only = only_here(folder, Path::new(from), stop)?;
    if !only.is_empty() {
        return Ok(State::Stays(format!(
            "It holds what isn't in {from} now: {}. Look at it before deleting it yourself.",
            listed(&only)
        )));
    }
    Ok(State::CutOff)
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

/// What's in the copy without its twin in `from` (at the same place, of the same kind), as paths inside it: leaving out
/// the marker at the top, a file the copy was writing under its temporary name (when its own file is in `from`), and
/// the files Explorer and Finder make by themselves. A folder in it that can't be read, or is deeper than walks go, is
/// named too: what's in it can't be told.
fn only_here(copy: &Path, from: &Path, stop: &AtomicBool) -> Result<Vec<String>, Stopped> {
    let shown = |p: &Path| text(p.strip_prefix(copy).unwrap_or(p));
    let mut only = Vec::new();
    let mut folders: Vec<(PathBuf, PathBuf, usize)> = vec![(copy.to_path_buf(), from.to_path_buf(), 0)];
    while let Some((dir, twin_dir, depth)) = folders.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            only.push(shown(&dir));
            continue;
        };
        for entry in entries {
            if stop.load(Ordering::Relaxed) {
                return Err(Stopped);
            }
            let Ok(entry) = entry else {
                only.push(shown(&dir));
                continue;
            };
            let name = entry.file_name();
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                only.push(shown(&path));
                continue;
            };
            if kind.is_file()
                && (depth == 0 && name == OsStr::new(MARKER) || made_by_the_system(&name.to_string_lossy()))
            {
                continue;
            }
            // A file the copy was writing over an older one when it stopped, named after a file still in `from`.
            let own_part = name.to_string_lossy().strip_suffix(PART_SUFFIX).map(|own| twin_dir.join(own));
            if kind.is_file() && own_part.is_some_and(|own| fs::symlink_metadata(own).is_ok_and(|m| m.is_file())) {
                continue;
            }
            let twin = twin_dir.join(&name);
            let twins = fs::symlink_metadata(&twin).is_ok_and(|m| {
                let t = m.file_type();
                if kind.is_symlink() {
                    t.is_symlink()
                } else if kind.is_dir() {
                    t.is_dir()
                } else {
                    t.is_file()
                }
            });
            if !twins {
                only.push(shown(&path));
            } else if kind.is_dir() {
                if depth + 1 >= DEEPEST {
                    only.push(shown(&path));
                } else {
                    folders.push((path, twin, depth + 1));
                }
            }
        }
    }
    only.sort();
    Ok(only)
}

/// Removes a copy the move made, for good, keeping its marker (if it has one) to the last: everything else in it goes
/// first, then the marker, then the folder. So a copy that can't all go (a file in it is open) keeps its marker, and is
/// cleared when it's tried again. Never a link, never through one: a link inside goes as the link.
pub(crate) fn remove_marked(folder: &Path) -> Result<(), String> {
    let couldnt = |p: &Path, e: io::Error| {
        format!("{} couldn't be removed ({e}): the rest of the half copy is cleared when it's tried again.", text(p))
    };
    if is_link(folder) || !real_folder(&text(folder)) {
        return Ok(());
    }
    let entries =
        fs::read_dir(folder).and_then(Iterator::collect::<io::Result<Vec<_>>>).map_err(|e| couldnt(folder, e))?;
    for entry in entries {
        if entry.file_name() == OsStr::new(MARKER) {
            continue;
        }
        let path = entry.path();
        remove_entry(&path).map_err(|e| couldnt(&path, e))?;
    }
    if !unmark(folder) {
        return Err(format!("The marker in {} couldn't be removed: it's tried again next time.", text(folder)));
    }
    fs::remove_dir(folder).map_err(|e| couldnt(folder, e))
}

/// Removes one thing in a copy the move made: a link as the link, a folder with all that's in it (links in it as
/// links, never followed), a file even when it's read-only (Windows won't remove those otherwise, and on a drive
/// without newer Windows' way of deleting, such as exFAT, not even inside a folder being removed).
fn remove_entry(path: &Path) -> io::Result<()> {
    let mut folders: Vec<(PathBuf, bool)> = Vec::new();
    remove_one(path, fs::symlink_metadata(path)?.file_type(), &mut folders)?;
    // Each folder is emptied, then removed once what's in it has gone.
    while let Some((dir, emptied)) = folders.pop() {
        if emptied {
            fs::remove_dir(&dir)?;
            continue;
        }
        folders.push((dir.clone(), true));
        for entry in fs::read_dir(&dir)?.collect::<io::Result<Vec<_>>>()? {
            remove_one(&entry.path(), entry.file_type()?, &mut folders)?;
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
