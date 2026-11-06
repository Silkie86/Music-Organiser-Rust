//! What the Space page's clean-ups put in the Recycle Bin (the Trash on a Mac or Linux), kept in a small file in the
//! app's folder. Each clean-up can then be put back for as long as it's still in the bin (the Space page's question 5:
//! "every clean-up result has a Put back button"), after a restart too, and the page can say what's still in the bin:
//! the app never empties it itself, and tells you what's there instead (Solomon, 3 Nov 2026: "leave it to me with a
//! warning"). The file only keeps what may still be in the bin (rule 28): each look in the bin drops what it no longer
//! holds, but for what was on a drive that isn't plugged in (its bin can't be looked in until it's back); and of the
//! clean-ups the bin can't vouch for (a Mac's Trash can't be looked in at all), at most [`KEEP`].
//!
//! A clean-up can take minutes (thousands of autosaves), so what it sends is noted as it goes, a line each, in a file
//! of its own beside the list ([`Going`]). Once the clean-up is kept in the list its note goes; a note left by a close
//! or a crash part way through is taken into the list by the next look ([`refresh`]), so what went can still be put
//! back, and the page's Recycle Bin line still counts it (the whole-app review of 6 Nov, E1-1).

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mo_space::clean::{Bin, Done, Removed, What};
use mo_space::in_bin::{self, InBin};
use mo_space::key;
use serde::{Deserialize, Serialize};

use crate::app_settings::write_checked;
use crate::log;
use crate::space_bin::{BIN, when_words};

/// The file, in the app's folder.
pub const FILE: &str = "space-binned.json";

/// Of the clean-ups the bin can't vouch for (it can't be looked in, or their drive isn't plugged in), at most this many
/// are kept: the oldest go first. What the bin still holds is always kept.
pub const KEEP: usize = 50;

/// How a running clean-up's note is named, beside [`FILE`]: this, its kind, `-`, when it ran, `-`, the number Windows
/// gave this run of the app (so two copies of the app running at once, or a run whose clock was put back, never write
/// into one another's), then [`NOTE_END`] ([`Going`]).
pub const NOTE_START: &str = "space-binned-going-";

/// How a running clean-up's note's name ends.
pub const NOTE_END: &str = ".jsonl";

/// One clean-up: which of the page's it was, when it ran, and what it sent to the bin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binned {
    /// Which clean-up: [`crate::space_spare::SPARE_COPIES`] for spare plugin copies.
    pub kind: String,
    /// When it ran (Unix seconds): with its kind, what Put back knows it by, so no two of a kind share one.
    pub when: i64,
    /// What went, with what the bin calls each: what Put back needs. What couldn't go isn't kept.
    pub done: Done,
    /// For the Plugins page's clean-ups and uninstalls ([`crate::space_bin::PLUGIN_CLEAN`],
    /// [`crate::space_bin::PLUGIN_UNINSTALL`]): the plugin's line, by its name key and own, so its Put back is on its
    /// card after a restart too (review X1-3). None for Space's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<(String, String)>,
}

impl Binned {
    /// How much room what it sent takes.
    pub fn bytes(&self) -> u64 {
        self.done.bytes()
    }

    /// The same clean-up with only those of what it sent that `keep` says, in the same order (a folder that went with
    /// another stays only while that one does: it comes back with it).
    fn only(&self, keep: &[bool]) -> Binned {
        let removed: Vec<Removed> =
            self.done.removed.iter().zip(keep).filter(|(_, keep)| **keep).map(|(r, _)| r.clone()).collect();
        let went_with =
            self.done.went_with.iter().filter(|(_, with)| removed.iter().any(|r| &r.path == with)).cloned().collect();
        let done = Done { removed, problems: self.done.problems.clone(), went_with };
        Binned { kind: self.kind.clone(), when: self.when, done, plugin: self.plugin.clone() }
    }
}

/// What's in the bin of the clean-ups kept, and what that comes to, for the page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    /// Which change to the list this is: of two, the higher saw the later list.
    pub seq: u64,
    /// What the bin holds now of each clean-up (all that's kept, where the bin can't be looked in), oldest first: what
    /// the page shows and puts back. What was on a drive that isn't plugged in stays in the file, for when it's back.
    pub list: Vec<Binned>,
    /// What that comes to, for the page's reminder.
    pub in_bin: InBin,
    /// Whether the list is kept in its file. When it couldn't be written, or read (said in the log), the app keeps it
    /// while it runs and tries again with the next change: what a clean-up sent can be put back from the page only
    /// until the app closes.
    pub noted: bool,
}

/// One change to the file at a time, so two can't lose each other's: the number of the last one, and each folder whose
/// file is behind what this run kept ([`Behind`]).
static CHANGES: Mutex<Changes> = Mutex::new(Changes { seq: 0, behind: None });

struct Changes {
    seq: u64,
    behind: Option<HashMap<PathBuf, Behind>>,
}

/// What a folder's file is missing.
enum Behind {
    /// It couldn't be written: the whole list as it was last kept, which the next change starts from, and writes.
    Unwritten(Vec<Binned>),
    /// It couldn't be read, so it isn't written over (what's in it would be lost): the clean-ups kept since, which go
    /// with what it holds once it can be read.
    Unread(Vec<Binned>),
}

/// When a clean-up runs (Unix seconds), as it's kept: a second later than the last one this run, at least, so two
/// clean-ups never have the same time, which is what Put back knows each one by ([`record`] sees to it across runs).
pub fn now() -> i64 {
    static LAST: Mutex<i64> = Mutex::new(0);
    let now =
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let mut last = LAST.lock().unwrap_or_else(PoisonError::into_inner);
    *last = now.max(last.saturating_add(1));
    *last
}

/// One line of a clean-up's note ([`Going`]): which clean-up (its kind and when it ran), and one thing it sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct NoteLine {
    kind: String,
    when: i64,
    removed: Removed,
}

/// Where the notes of the clean-ups running in this run of the app are: they aren't taken into the list while they run
/// (each is kept there whole once it ends). Another copy of the app running at once tells its own apart by the lock it
/// holds on each ([`Going::went`]).
static RUNNING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// A running clean-up's note of what it has sent, beside [`FILE`] in the app's folder: each thing is added the moment
/// it goes ([`Going::went`], told by [`mo_space::clean::Bin::going`]), a line of its own, so however the app ends after,
/// what went is on the disk (a power cut can still lose the last few lines: they aren't waited on, as thousands of
/// waits would slow Remove down). The file is made when the first thing goes (a clean-up that sends nothing leaves
/// none), and taken away once the clean-up is kept in the list ([`Going::finish`]). One left behind is taken into the
/// list by the next look ([`refresh`]).
pub struct Going {
    kind: String,
    when: i64,
    path: PathBuf,
    note: Mutex<Note>,
}

/// What a note holds while its clean-up runs: the file, once made (locked while it's open), and what went.
#[derive(Default)]
struct Note {
    file: Option<fs::File>,
    /// Set once the file couldn't be made or written to (said in the log once): from then on what goes is only kept
    /// here, for [`Going::went_so_far`].
    failed: bool,
    went: Vec<Removed>,
}

impl Going {
    /// The note of a clean-up of `kind` that runs at `when` ([`now`]), in `folder`.
    pub fn start(folder: &Path, kind: &str, when: i64) -> Going {
        let path = note_path(folder, kind, when);
        RUNNING.lock().unwrap_or_else(PoisonError::into_inner).push(path.clone());
        Going { kind: kind.to_owned(), when, path, note: Mutex::default() }
    }

    /// Notes one more thing on its way to the bin, a line written at once. What can't be written (said in the log) is
    /// still kept here.
    pub fn went(&self, removed: &Removed) {
        let mut note = self.note.lock().unwrap_or_else(PoisonError::into_inner);
        note.went.push(removed.clone());
        if note.failed {
            return;
        }
        let line = NoteLine { kind: self.kind.clone(), when: self.when, removed: removed.clone() };
        let written = serde_json::to_string(&line).map_err(io::Error::other).and_then(|mut text| {
            text.push('\n');
            let file = match note.file.take() {
                Some(file) => file,
                None => {
                    if let Some(folder) = self.path.parent() {
                        fs::create_dir_all(folder)?;
                    }
                    let file = open_note(&self.path)?;
                    // Held while the clean-up runs, so another copy of the app running at once leaves it be. Where
                    // files can't be locked (said in the log), this run's own looks still do (`RUNNING`).
                    if let Err(fs::TryLockError::Error(e)) = file.try_lock() {
                        let at = self.path.display();
                        log::warning("Space", &format!("Couldn't lock the note of what goes to the {BIN} ({at}): {e}"));
                    }
                    file
                }
            };
            let file = note.file.insert(file);
            file.write_all(text.as_bytes())
        });
        if let Err(e) = written {
            note.failed = true;
            note.file = None;
            let at = self.path.display();
            log::warning("Space", &format!("Couldn't note what goes to the {BIN} as it goes ({at}): {e}"));
        }
    }

    /// What it noted so far, in the order it went: what had gone when a clean-up stopped with an error.
    pub fn went_so_far(&self) -> Vec<Removed> {
        self.note.lock().unwrap_or_else(PoisonError::into_inner).went.clone()
    }

    /// The clean-up has ended: once it's `kept` in the list ([`Kept::noted`]), its note goes. When it couldn't be kept
    /// there, the note stays for the next look to take in.
    pub fn finish(self, kept: bool) {
        // Closed (and so let go) first: a file that's open may not be taken away.
        self.note.lock().unwrap_or_else(PoisonError::into_inner).file = None;
        if kept {
            remove_note(&self.path);
        }
    }
}

impl Drop for Going {
    fn drop(&mut self) {
        let mut running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = running.iter().position(|path| *path == self.path) {
            running.swap_remove(at);
        }
    }
}

/// Opens a note to add to, made if it isn't there yet. It's opened to read as well: Windows locks a file only through
/// a handle that may read or write it, and one that may only add to the end is neither.
fn open_note(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new().create(true).read(true).append(true).open(path)
}

/// Where the note of a clean-up of `kind` that ran at `when` in this run of the app is kept in `folder`.
fn note_path(folder: &Path, kind: &str, when: i64) -> PathBuf {
    folder.join(format!("{NOTE_START}{kind}-{when}-{}{NOTE_END}", std::process::id()))
}

/// A note that can't be read for this long (since it last changed) is taken away, so it doesn't stay for ever (rule
/// 28): what it names stays in the bin, to put back from there.
const UNREADABLE_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Takes away a note that's done with, unless another copy of the app running at once holds it locked: its clean-up is
/// running there (it was taken in just as it started), and keeps it all itself ([`add`]). One that's already gone is
/// fine; one that can't be taken away (said in the log) is taken in again by a later look, which adds nothing twice.
fn remove_note(path: &Path) {
    let written = fs::File::open(path).is_ok_and(|file| matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock)));
    if written {
        return;
    }
    if let Err(e) = fs::remove_file(path)
        && e.kind() != io::ErrorKind::NotFound
    {
        log::warning(
            "Space",
            &format!("Couldn't take away a note of what went to the {BIN} ({}): {e}", path.display()),
        );
    }
}

/// The notes in `folder` ([`Going`]), each with what it holds, but for those of clean-ups still running (in this run of
/// the app, or another copy of it running at once): what a close or a crash part way through left, or a clean-up whose
/// list couldn't be written. A line a crash cut short, or one that can't be read, is left out (said in the log). Reads
/// the folder and each note: call it off the window's thread.
fn notes_in(folder: &Path) -> Vec<(PathBuf, Vec<NoteLine>)> {
    let running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
    let mut notes = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let is_note = name.to_str().is_some_and(|n| n.starts_with(NOTE_START) && n.ends_with(NOTE_END));
        let path = entry.path();
        if !is_note || running.contains(&path) || !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let read = fs::File::open(&path).and_then(|file| {
            // Locked by the clean-up writing it, in another copy of the app: it's still running. (Where files can't be
            // locked, it's read.)
            if matches!(file.try_lock_shared(), Err(fs::TryLockError::WouldBlock)) {
                return Ok(None);
            }
            io::read_to_string(file).map(Some)
        });
        let text = match read {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(e) => {
                let at = path.display();
                let age = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok());
                if age.is_some_and(|age| age >= UNREADABLE_FOR) {
                    log::warning(
                        "Space",
                        &format!(
                            "A note of what went to the {BIN} couldn't be read for a month, so it's taken away: what it \
                             names stays in the {BIN}, to put back from there ({at}): {e}"
                        ),
                    );
                    remove_note(&path);
                } else {
                    log::warning("Space", &format!("Couldn't read a note of what went to the {BIN} ({at}): {e}"));
                }
                continue;
            }
        };
        let mut unread = 0;
        let lines: Vec<NoteLine> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).map_err(|_| unread += 1).ok())
            .collect();
        if unread > 0 {
            let at = path.display();
            log::warning(
                "Space",
                &format!("{unread} lines of a note of what went to the {BIN} couldn't be read ({at})"),
            );
        }
        notes.push((path, lines));
    }
    notes
}

/// Takes into `list` (oldest first) what `notes` say went: each thing the list doesn't have yet (the same thing, gone
/// at the same time), with its clean-up when the list has some of that already (a note left beside its clean-up: by a
/// crash, or because it couldn't be taken away), else as a clean-up of its own, in its place by time (a second or more
/// later when another of its kind is kept under that time: [`free_time`]). Never with another clean-up because it has
/// the same time: the clock may have been put back since that one.
fn take_in(list: &mut Vec<Binned>, notes: &[(PathBuf, Vec<NoteLine>)]) {
    let pair = |r: &Removed| (key(&r.real), r.when);
    // Which clean-up has each thing kept (none for one this takes in as a clean-up of its own).
    let mut have: HashMap<(String, i64), Option<usize>> = HashMap::new();
    for (at, binned) in list.iter().enumerate() {
        for r in &binned.done.removed {
            have.insert(pair(r), Some(at));
        }
    }
    let mut own = Vec::new();
    for (_, lines) in notes {
        let Some(NoteLine { kind, when, .. }) = lines.first() else { continue };
        let with = lines.iter().find_map(|line| match have.get(&pair(&line.removed)) {
            Some(Some(at)) if list.get(*at).is_some_and(|b| b.kind == *kind) => Some(*at),
            _ => None,
        });
        let mut new = Vec::new();
        for line in lines {
            if let Entry::Vacant(spot) = have.entry(pair(&line.removed)) {
                spot.insert(None);
                new.push(line.removed.clone());
            }
        }
        match with.and_then(|at| list.get_mut(at)) {
            _ if new.is_empty() => {}
            Some(binned) => binned.done.removed.extend(new),
            // A note's own clean-up is Space's: the Plugins page's don't write notes yet.
            None => own.push(Binned {
                kind: kind.clone(),
                when: *when,
                done: Done { removed: new, ..Done::default() },
                plugin: None,
            }),
        }
    }
    for binned in own {
        let when = free_time(list, &binned.kind, binned.when);
        let at = list.iter().position(|b| b.when > when).unwrap_or(list.len());
        list.insert(at, Binned { when, ..binned });
    }
}

/// Where the plugin files the app's clean-ups put in the bin were, as kept in `folder` and in the notes of clean-ups
/// cut short ([`Going`]): those that aren't back in their place. The Plugins page leaves them out of what it counts, as
/// the Space page does, even before the Space page has looked in the bin this run (the whole-app review of 6 Nov,
/// E1-4). Reads the list and the notes and looks at each place on the disk, but not in the bin: call it off the
/// window's thread.
pub fn plugin_files(folder: &Path) -> Vec<String> {
    let kept = read(folder).unwrap_or_else(|e| {
        log::warning("Space", &format!("Couldn't read what the app put in the {BIN} ({FILE}): {e}"));
        Vec::new()
    });
    let kept = kept.into_iter().flat_map(|b| b.done.removed);
    let noted = notes_in(folder).into_iter().flat_map(|(_, lines)| lines).map(|line| line.removed);
    let mut seen = HashSet::new();
    kept.chain(noted)
        .filter(|r| matches!(r.what, What::Plugin(_)))
        .filter(|r| fs::symlink_metadata(&r.path).is_err() && seen.insert(key(&r.path)))
        .map(|r| r.path)
        .collect()
}

/// The clean-ups kept in `folder`, oldest first: none when there's no file yet, and none from a damaged one (said in
/// the log), which the next clean-up writes over. Err when it's there but can't be read (another program has it open,
/// say): then it mustn't be written over.
pub fn read(folder: &Path) -> io::Result<Vec<Binned>> {
    let text = match fs::read_to_string(folder.join(FILE)) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(serde_json::from_str(&text).unwrap_or_else(|e| {
        log::warning("Space", &format!("What the app put in the {BIN} ({FILE}) couldn't be read: {e}"));
        Vec::new()
    }))
}

/// Keeps `list` in `folder`, as the app's settings are kept ([`write_checked`]: written whole beside the file first,
/// read back, then put in its place, so a crash part way leaves the old one whole). With nothing to keep, the file
/// goes. Waits for the disk: call it off the window's thread.
pub fn write(folder: &Path, list: &[Binned]) -> io::Result<()> {
    let path = folder.join(FILE);
    if list.is_empty() {
        return match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let json = serde_json::to_string(list).map_err(io::Error::other)?;
    fs::create_dir_all(folder)?;
    write_checked(&path, &json)
}

/// Adds a clean-up of `kind` that ran at `when`: only what went, and nothing when nothing did. The newest goes last.
/// What of it the list has already, taken in from its note while it ran (by another copy of the app running at once:
/// kept under its kind and time), is kept once, as this says it: the note's copy goes, with a clean-up that's left with
/// nothing. Gives the time it's kept under: `when`, or a second or more later when another of its kind is kept under
/// that time (the computer's clock was put back since), as Put back knows each one by its kind and time.
pub fn add(list: &mut Vec<Binned>, kind: &str, when: i64, done: &Done) -> i64 {
    add_for(list, kind, when, done, None)
}

/// [`add`], for the plugin line `plugin` (the Plugins page's clean-ups).
fn add_for(list: &mut Vec<Binned>, kind: &str, when: i64, done: &Done, plugin: Option<(String, String)>) -> i64 {
    if !done.removed.is_empty() && list.iter().any(|b| b.kind == kind && b.when == when) {
        let its: HashSet<(String, i64)> = done.removed.iter().map(|r| (key(&r.real), r.when)).collect();
        list.retain_mut(|b| {
            if b.kind != kind || b.when != when {
                return true;
            }
            let before = b.done.removed.len();
            b.done.removed.retain(|r| !its.contains(&(key(&r.real), r.when)));
            before == b.done.removed.len() || !b.done.removed.is_empty()
        });
    }
    let when = free_time(list, kind, when);
    if !done.removed.is_empty() {
        let done = Done { removed: done.removed.clone(), problems: Vec::new(), went_with: done.went_with.clone() };
        list.push(Binned { kind: kind.to_owned(), when, done, plugin });
    }
    when
}

/// `when`, or the first second after it no clean-up of `kind` in `list` is kept under.
fn free_time(list: &[Binned], kind: &str, mut when: i64) -> i64 {
    while list.iter().any(|b| b.kind == kind && b.when == when) {
        match when.checked_add(1) {
            Some(next) => when = next,
            None => break,
        }
    }
    when
}

/// What a look in the bin found of the clean-ups kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Looked {
    /// What to keep in the file: what the bin still holds, and what was on a drive that isn't plugged in; of the
    /// clean-ups the bin can't vouch for, at most [`KEEP`].
    pub keep: Vec<Binned>,
    /// What the bin holds now (all that's kept, where it can't be looked in): what the page shows and puts back.
    pub shown: Vec<Binned>,
    /// What that comes to.
    pub in_bin: InBin,
}

/// Looks in the bin for the clean-ups in `list` (oldest first): each keeps only what the bin still holds (not once
/// you've emptied the bin, or put it back), but for what was on a drive that isn't plugged in, which stays in the file
/// for when it's back; one with nothing left goes. When the bin can't be looked in (a Mac's Trash), everything stays.
/// Looks in the bin once, and not at all for an empty list: call it away from the window's thread.
pub fn look(list: Vec<Binned>, bin: &dyn Bin) -> Looked {
    if list.is_empty() {
        return Looked::default();
    }
    let removed: Vec<Removed> = list.iter().flat_map(|b| b.done.removed.iter().cloned()).collect();
    let Some(held) = bin.holds(&removed) else {
        let keep = trim(list.into_iter().map(|b| (b, false)).collect());
        let in_bin = in_bin::remind_held(keep.iter().flat_map(|b| &b.done.removed), None);
        return Looked { shown: keep.clone(), keep, in_bin };
    };
    let in_bin = in_bin::remind_held(&removed, Some(&held));
    let (mut keep, mut shown, mut at) = (Vec::new(), Vec::new(), 0);
    for binned in list {
        let n = binned.done.removed.len();
        let held: Vec<bool> = (at..at + n).map(|i| held.get(i).copied().unwrap_or(false)).collect();
        at += n;
        // What the bin doesn't hold stays only while its drive isn't plugged in: the bin can't be looked in for it.
        let kept: Vec<bool> =
            binned.done.removed.iter().zip(&held).map(|(r, held)| *held || !bin.can_look_for(r)).collect();
        let vouched = held.contains(&true);
        if vouched {
            shown.push(binned.only(&held));
        }
        if kept.contains(&true) {
            keep.push((binned.only(&kept), vouched));
        }
    }
    Looked { keep: trim(keep), shown, in_bin }
}

/// The clean-ups to keep, of `list` (oldest first, each with whether the bin vouches for it: it holds some of what it
/// sent): every one it vouches for, and of the others, the newest while there are more than [`KEEP`] in all. Each one
/// that goes is said in the log.
fn trim(list: Vec<(Binned, bool)>) -> Vec<Binned> {
    let mut over = list.len().saturating_sub(KEEP);
    let mut out = Vec::with_capacity(list.len());
    for (binned, vouched) in list {
        if over > 0 && !vouched {
            over -= 1;
            log::info(
                "Space",
                &format!(
                    "Stopped keeping track of the clean-up of {} ({} things): the {BIN} can't say if they're still in \
                     it, and only {KEEP} such clean-ups are kept.",
                    when_words(binned.when),
                    binned.done.removed.len()
                ),
            );
        } else {
            out.push(binned);
        }
    }
    out
}

/// Reads the clean-ups kept in `folder`, lets `change` change them, takes in what the notes of clean-ups cut short say
/// went ([`Going`]), keeps what may still be in the bin ([`look`]), and writes them back when that changed them. One at
/// a time, so two changes can't lose each other's. A list that couldn't be written (said in the log) is kept here
/// instead while the app runs: the next change starts from it, not the file, and writes it. A file that couldn't be
/// read isn't written over: what's added meanwhile is kept here, and goes with what it holds once it can be read.
/// Looks in the bin and waits for the disk: call it off the window's thread.
fn update(folder: &Path, bin: &dyn Bin, change: impl FnOnce(&mut Vec<Binned>)) -> Kept {
    let mut changes = CHANGES.lock().unwrap_or_else(PoisonError::into_inner);
    changes.seq += 1;
    let seq = changes.seq;
    let behind = changes.behind.as_mut().and_then(|b| b.remove(folder));
    // What the file holds, as far as this run knows (none when the list here is ahead of it), and the list to change.
    let (file, mut list) = match behind {
        Some(Behind::Unwritten(list)) => (None, list),
        unread => match read(folder) {
            Ok(file) => {
                let mut list = file.clone();
                if let Some(Behind::Unread(since)) = unread {
                    for b in since {
                        let when = free_time(&list, &b.kind, b.when);
                        list.push(Binned { when, ..b });
                    }
                }
                (Some(file), list)
            }
            Err(e) => {
                log::warning("Space", &format!("Couldn't read what the app put in the {BIN} ({FILE}): {e}"));
                let mut since = match unread {
                    Some(Behind::Unread(since)) => since,
                    _ => Vec::new(),
                };
                change(&mut since);
                let looked = look(since, bin);
                changes.behind.get_or_insert_with(HashMap::new).insert(folder.to_owned(), Behind::Unread(looked.keep));
                return Kept { seq, list: looked.shown, in_bin: looked.in_bin, noted: false };
            }
        },
    };
    change(&mut list);
    // What clean-ups cut short noted as they went (or one whose list couldn't be kept): taken in, their notes go once
    // it's all kept. While the file can't be read (above) they wait where they are.
    let notes = notes_in(folder);
    if !notes.is_empty() {
        take_in(&mut list, &notes);
    }
    let looked = look(list, bin);
    let noted = file.as_ref() == Some(&looked.keep)
        || write(folder, &looked.keep)
            .map_err(|e| log::warning("Space", &format!("Couldn't keep what the app put in the {BIN} ({FILE}): {e}")))
            .is_ok();
    if noted {
        for (path, _) in &notes {
            remove_note(path);
        }
    } else {
        changes.behind.get_or_insert_with(HashMap::new).insert(folder.to_owned(), Behind::Unwritten(looked.keep));
    }
    Kept { seq, list: looked.shown, in_bin: looked.in_bin, noted }
}

/// What the app's clean-ups put in the bin that's still there, as kept in `folder` (and what's gone from the bin no
/// longer kept). Looks in the bin when anything is kept: call it off the window's thread.
pub fn refresh(folder: &Path, bin: &dyn Bin) -> Kept {
    update(folder, bin, |_| {})
}

/// Keeps a clean-up of `kind` that ran at `when` in `folder` ([`add`]), and says what's kept now, and the time it's
/// kept under ([`add`] says which). Looks in the bin and waits for the disk: call it off the window's thread.
pub fn record(folder: &Path, bin: &dyn Bin, kind: &str, when: i64, done: &Done) -> (Kept, i64) {
    record_for(folder, bin, kind, when, done, None)
}

/// [`record`], for a clean-up of the plugin line `plugin` (the Plugins page's Clean up and Uninstall: review X1-3).
pub fn record_for(
    folder: &Path,
    bin: &dyn Bin,
    kind: &str,
    when: i64,
    done: &Done,
    plugin: Option<(String, String)>,
) -> (Kept, i64) {
    let mut kept_at = when;
    let kept = update(folder, bin, |list| kept_at = add_for(list, kind, when, done, plugin));
    (kept, kept_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_space::clean::{Problem, What};
    use mo_space::plugin_space::SpaceKind;
    use std::cell::RefCell;
    use std::path::PathBuf;

    fn removed(path: &str, bytes: u64) -> Removed {
        Removed {
            path: path.to_owned(),
            real: path.to_owned(),
            bytes,
            what: What::Plugin(SpaceKind::Program),
            when: 100,
            bin_id: Some(format!("bin:{path}")),
            binned: None,
            binned_zone: None,
            stamp: None,
        }
    }

    fn done(paths: &[&str]) -> Done {
        Done { removed: paths.iter().map(|p| removed(p, 10)).collect(), ..Done::default() }
    }

    /// A bin that holds what it's told (it can't be looked in when told nothing), and can't be looked in for what's on
    /// a drive that isn't plugged in (`away`); it counts its looks.
    struct Holding {
        held: Option<Vec<&'static str>>,
        away: Vec<&'static str>,
        looks: RefCell<usize>,
    }

    impl Bin for Holding {
        fn send(&self, _path: &Path) -> Result<(), String> {
            Ok(())
        }
        fn find(&self, _removed: &mut [Removed]) -> Vec<Problem> {
            Vec::new()
        }
        fn put_back(&self, _removed: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
        fn holds(&self, removed: &[Removed]) -> Option<Vec<bool>> {
            *self.looks.borrow_mut() += 1;
            let held = self.held.as_ref()?;
            Some(removed.iter().map(|r| held.contains(&r.path.as_str())).collect())
        }
        fn can_look_for(&self, removed: &Removed) -> bool {
            !self.away.contains(&removed.path.as_str())
        }
    }

    fn holding(held: Option<Vec<&'static str>>) -> Holding {
        Holding { held, away: Vec::new(), looks: RefCell::new(0) }
    }

    fn whens(list: &[Binned]) -> Vec<i64> {
        list.iter().map(|b| b.when).collect()
    }

    /// A folder for one test, made new: never the app's real folder (rule 11).
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn what_went_is_kept_and_read_back() {
        let dir = folder("mo-app-binned-kept");
        assert!(read(&dir).unwrap().is_empty());
        let mut list = Vec::new();
        let mut went = done(&["C:/a.dll", "C:/b.dll"]);
        went.problems.push(Problem { path: "C:/c.dll".to_owned(), why: "It changed since it was found".to_owned() });
        assert_eq!(add(&mut list, "spare-copies", 1_762_268_400, &went), 1_762_268_400);
        // A clean-up that sent nothing isn't kept.
        add(&mut list, "spare-copies", 1_762_268_500, &Done::default());
        write(&dir, &list).unwrap();
        let back = read(&dir).unwrap();
        assert_eq!(back, list);
        assert_eq!((back.len(), back[0].bytes()), (1, 20));
        // Only what went: what stayed isn't anything to put back.
        assert!(back[0].done.problems.is_empty());
        assert!(!dir.join(format!("{FILE}{}", crate::backup::PART_ENDING)).exists());
        // Nothing left: the file goes.
        write(&dir, &[]).unwrap();
        assert!(!dir.join(FILE).exists());
        write(&dir, &[]).unwrap();
    }

    /// Review X1-3: a Plugins page clean-up is kept with its plugin's line, read back with it; one kept before (no
    /// plugin in the file) still reads, and Space's own aren't written with one.
    #[test]
    fn a_plugin_clean_up_keeps_its_plugin() {
        let dir = folder("mo-app-binned-plugin");
        let mut list = Vec::new();
        add(&mut list, "spare-copies", 1_762_268_400, &done(&["C:/a.dll"]));
        let plugin = Some(("serum".to_owned(), String::new()));
        add_for(&mut list, crate::space_bin::PLUGIN_CLEAN, 1_762_268_401, &done(&["C:/Serum"]), plugin.clone());
        write(&dir, &list).unwrap();
        let text = fs::read_to_string(dir.join(FILE)).unwrap();
        assert_eq!(text.matches("\"plugin\"").count(), 1, "{text}");
        let back = read(&dir).unwrap();
        assert_eq!(back, list);
        assert_eq!(back[1].plugin, plugin);
        assert_eq!(back[0].plugin, None);
    }

    #[test]
    fn a_damaged_file_counts_as_none() {
        let dir = folder("mo-app-binned-damaged");
        fs::write(dir.join(FILE), "[{\"kind\": 3").unwrap();
        assert!(read(&dir).unwrap().is_empty());
        // A look doesn't write over it; a clean-up does.
        let bin = holding(Some(vec!["C:/a.dll"]));
        assert_eq!(refresh(&dir, &bin).list, []);
        assert_eq!(fs::read_to_string(dir.join(FILE)).unwrap(), "[{\"kind\": 3");
        let (kept, when) = record(&dir, &bin, "spare-copies", 1, &done(&["C:/a.dll"]));
        assert!(kept.noted && kept.list.len() == 1 && when == 1);
        assert_eq!(read(&dir).unwrap(), kept.list);
    }

    #[test]
    fn a_file_that_cant_be_read_isnt_written_over() {
        let dir = folder("mo-app-binned-unread");
        // A folder where the file should be: it's there, but can't be read (as when another program has it open).
        fs::create_dir_all(dir.join(FILE)).unwrap();
        assert!(read(&dir).is_err());
        let bin = holding(Some(vec!["C:/a.dll", "C:/b.dll"]));
        let (kept, when) = record(&dir, &bin, "spare-copies", 2, &done(&["C:/b.dll"]));
        // What went is kept here, for Put back, and not written over what's there.
        assert_eq!((whens(&kept.list), kept.noted, when), (vec![2], false, 2));
        assert!(dir.join(FILE).is_dir());
        // Once it can be read, what it holds and what went since are kept together.
        fs::remove_dir(dir.join(FILE)).unwrap();
        let mut earlier = Vec::new();
        add(&mut earlier, "spare-copies", 1, &done(&["C:/a.dll"]));
        write(&dir, &earlier).unwrap();
        let now = refresh(&dir, &bin);
        assert_eq!((whens(&now.list), now.noted), (vec![1, 2], true));
        assert_eq!(read(&dir).unwrap(), now.list);
        assert_eq!(now.in_bin.items, 2);
    }

    #[test]
    fn only_what_the_bin_cant_vouch_for_is_cut_to_keep() {
        let mut list = Vec::new();
        for n in 0..KEEP as i64 + 5 {
            add(&mut list, "spare-copies", n, &done(&[if n == 0 { "C:/first.dll" } else { "C:/a.dll" }]));
        }
        // What the bin still holds is all kept, however many.
        let all = look(list.clone(), &holding(Some(vec!["C:/first.dll", "C:/a.dll"])));
        assert_eq!((all.keep.len(), all.shown.len()), (KEEP + 5, KEEP + 5));
        // A bin that can't be looked in (a Mac's Trash): the newest of them.
        let mac = look(list.clone(), &holding(None));
        assert_eq!((mac.keep.len(), whens(&mac.keep).first().copied()), (KEEP, Some(5)));
        assert_eq!(mac.shown, mac.keep);
        // On a drive that isn't plugged in, only the first is still known to be in the bin: it stays, and of the
        // others, the newest.
        let unplugged = Holding { held: Some(vec!["C:/first.dll"]), away: vec!["C:/a.dll"], looks: RefCell::new(0) };
        let found = look(list, &unplugged);
        assert_eq!(found.keep.len(), KEEP);
        assert_eq!(whens(&found.keep)[..2], [0, 6]);
        assert_eq!(whens(&found.shown), [0]);
    }

    #[test]
    fn what_the_bin_no_longer_holds_goes() {
        let mut list = Vec::new();
        add(&mut list, "spare-copies", 1, &done(&["C:/a.dll", "C:/b.dll"]));
        add(&mut list, "spare-copies", 2, &done(&["C:/c.dll"]));
        // a.dll was put back and c.dll emptied from the bin: only b.dll is left, in the first clean-up.
        let bin = holding(Some(vec!["C:/b.dll"]));
        let found = look(list.clone(), &bin);
        assert_eq!(found.keep, found.shown);
        assert_eq!(found.shown.len(), 1);
        assert_eq!(found.shown[0].done.removed.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["C:/b.dll"]);
        // What's still there, for the reminder, from the same look: the bin is looked in once for all of it.
        assert_eq!((found.in_bin.items, found.in_bin.bytes, found.in_bin.looked), (1, 10, true));
        assert_eq!(*bin.looks.borrow(), 1);
        // A bin that can't be looked in keeps them all, and says so.
        let all = look(list.clone(), &holding(None));
        assert_eq!((all.keep, all.shown), (list.clone(), list.clone()));
        assert_eq!((all.in_bin.items, all.in_bin.looked), (3, false));
        // Nothing kept: the bin isn't looked in at all.
        let bin = holding(Some(Vec::new()));
        assert_eq!(look(Vec::new(), &bin), Looked::default());
        assert_eq!(*bin.looks.borrow(), 0);
    }

    #[test]
    fn whats_on_a_drive_that_isnt_plugged_in_stays_for_when_its_back() {
        let mut list = Vec::new();
        add(&mut list, "spare-copies", 1, &done(&["C:/a.dll", "I:/b.dll"]));
        add(&mut list, "spare-copies", 2, &done(&["I:/c.dll"]));
        // I: isn't plugged in: its bin isn't listed, though b.dll and c.dll may still be in it.
        let bin = Holding { held: Some(vec!["C:/a.dll"]), away: vec!["I:/b.dll", "I:/c.dll"], looks: RefCell::new(0) };
        let found = look(list.clone(), &bin);
        assert_eq!(found.keep, list);
        // The page shows, and puts back, what's in the bin now.
        assert_eq!(whens(&found.shown), [1]);
        assert_eq!(found.shown[0].done.removed.len(), 1);
        assert_eq!((found.in_bin.items, found.in_bin.bytes), (1, 10));
        // Back, with c.dll emptied from it: only b.dll stays.
        let back = look(found.keep, &holding(Some(vec!["C:/a.dll", "I:/b.dll"])));
        assert_eq!(whens(&back.keep), [1]);
        assert_eq!(back.keep[0].done.removed.len(), 2);
    }

    #[test]
    fn a_folder_that_went_with_another_stays_with_it() {
        let mut went = done(&["C:/Kits", "C:/Loops"]);
        went.went_with.push(("C:/Kits/Drums".to_owned(), "C:/Kits".to_owned()));
        let mut list = Vec::new();
        add(&mut list, "spare-copies", 1, &went);
        let found = look(list.clone(), &holding(Some(vec!["C:/Kits"])));
        assert_eq!(found.shown[0].done.went_with.len(), 1);
        let found = look(list, &holding(Some(vec!["C:/Loops"])));
        assert!(found.shown[0].done.went_with.is_empty());
    }

    #[test]
    fn each_change_is_kept_and_numbered() {
        let dir = folder("mo-app-binned-changes");
        let bin = holding(Some(vec!["C:/a.dll", "C:/b.dll"]));
        let (first, _) = record(&dir, &bin, "spare-copies", 1, &done(&["C:/a.dll"]));
        let (second, _) = record(&dir, &bin, "spare-copies", 2, &done(&["C:/b.dll"]));
        assert!(second.seq > first.seq);
        assert_eq!(whens(&second.list), [1, 2]);
        assert_eq!((second.in_bin.items, second.in_bin.bytes), (2, 20));
        assert_eq!(read(&dir).unwrap(), second.list);
        // a.dll was put back: a look drops it from the file too.
        let now = refresh(&dir, &holding(Some(vec!["C:/b.dll"])));
        assert!(now.seq > second.seq && now.noted);
        assert_eq!(whens(&now.list), [2]);
        assert_eq!(read(&dir).unwrap(), now.list);
        // Everything gone from the bin: so is the file.
        let none = refresh(&dir, &holding(Some(Vec::new())));
        assert!(none.list.is_empty() && none.in_bin.is_empty());
        assert!(!dir.join(FILE).exists());
    }

    #[test]
    fn a_clean_up_at_a_time_already_kept_gets_a_later_one() {
        // The clock was put back since the first: the second is kept a second later, as Put back knows each by it.
        let dir = folder("mo-app-binned-same-time");
        let bin = holding(Some(vec!["C:/a.dll", "C:/b.dll", "C:/c.dll"]));
        let (_, first) = record(&dir, &bin, "spare-copies", 7, &done(&["C:/a.dll"]));
        let (_, second) = record(&dir, &bin, "spare-copies", 7, &done(&["C:/b.dll"]));
        let (kept, third) = record(&dir, &bin, "spare-copies", 7, &done(&["C:/c.dll"]));
        assert_eq!((first, second, third), (7, 8, 9));
        assert_eq!(whens(&kept.list), [7, 8, 9]);
        // Another kind of clean-up can share it.
        let mut list = kept.list;
        assert_eq!(add(&mut list, "old-copies", 7, &done(&["C:/d"])), 7);
    }

    #[test]
    fn a_list_that_couldnt_be_written_is_kept_until_it_can_be() {
        let dir = folder("mo-app-binned-unwritten");
        // The app's folder is a file here, so nothing can be written in it.
        let blocked = dir.join("app");
        fs::write(&blocked, "not a folder").unwrap();
        let bin = holding(Some(vec!["C:/a.dll", "C:/b.dll"]));
        let (first, _) = record(&blocked, &bin, "spare-copies", 1, &done(&["C:/a.dll"]));
        assert!(!first.noted && first.list.len() == 1);
        // The next look starts from what was kept, not the file it couldn't write, and tries again.
        let (second, _) = record(&blocked, &bin, "spare-copies", 2, &done(&["C:/b.dll"]));
        assert!(!second.noted);
        assert_eq!(whens(&second.list), [1, 2]);
        let looked = refresh(&blocked, &bin);
        assert_eq!((looked.list.len(), looked.noted), (2, false));
        // Once it can be written, it is, whole.
        fs::remove_file(&blocked).unwrap();
        fs::create_dir_all(&blocked).unwrap();
        let written = refresh(&blocked, &bin);
        assert!(written.noted);
        assert_eq!(read(&blocked).unwrap(), second.list);
        assert!(refresh(&blocked, &bin).noted);
    }

    /// What a clean-up sends, as it's noted going: not yet found in the bin.
    fn going(path: &str, when: i64) -> Removed {
        Removed { when, bin_id: None, ..removed(path, 10) }
    }

    fn paths(binned: &Binned) -> Vec<&str> {
        binned.done.removed.iter().map(|r| r.path.as_str()).collect()
    }

    fn notes(dir: &Path) -> Vec<PathBuf> {
        let mut notes: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(NOTE_START)))
            .collect();
        notes.sort();
        notes
    }

    #[test]
    fn what_a_clean_up_cut_short_sent_is_kept_by_the_next_look() {
        let dir = folder("mo-app-binned-cut-short");
        let bin = holding(Some(vec!["C:/a.dll", "C:/b.dll"]));
        let note = Going::start(&dir, "spare-copies", 5);
        // Nothing sent yet: no note.
        assert!(notes(&dir).is_empty());
        note.went(&going("C:/a.dll", 5));
        note.went(&going("C:/b.dll", 6));
        assert_eq!(notes(&dir).len(), 1);
        assert_eq!(note.went_so_far().len(), 2);
        // While it runs, a look leaves its note be: it's kept whole once it ends.
        let looked = refresh(&dir, &bin);
        assert!(looked.list.is_empty() && looked.noted);
        assert_eq!(notes(&dir).len(), 1);
        // The app closes (or crashes) before it's kept: the next look keeps what went, and the note goes.
        drop(note);
        let kept = refresh(&dir, &bin);
        assert!(kept.noted);
        assert_eq!((whens(&kept.list), paths(&kept.list[0])), (vec![5], vec!["C:/a.dll", "C:/b.dll"]));
        assert_eq!((kept.in_bin.items, kept.in_bin.bytes), (2, 20));
        assert_eq!(read(&dir).unwrap(), kept.list);
        assert!(notes(&dir).is_empty());
        // Looked at again, nothing is added twice.
        assert_eq!(refresh(&dir, &bin).list, kept.list);
    }

    #[test]
    fn a_clean_up_kept_whole_takes_its_note_away() {
        let dir = folder("mo-app-binned-note-kept");
        let bin = holding(Some(vec!["C:/a.dll", "C:/b.dll"]));
        let note = Going::start(&dir, "spare-copies", 7);
        let went = Done { removed: vec![going("C:/a.dll", 7), going("C:/b.dll", 8)], ..Done::default() };
        for r in &went.removed {
            note.went(r);
        }
        let (kept, _) = record(&dir, &bin, "spare-copies", 7, &went);
        assert!(kept.noted);
        note.finish(kept.noted);
        assert!(notes(&dir).is_empty());
        // One whose list couldn't be kept leaves its note for the next look.
        let note = Going::start(&dir, "spare-copies", 9);
        note.went(&going("C:/a.dll", 9));
        note.finish(false);
        assert_eq!(notes(&dir).len(), 1);
    }

    #[test]
    fn a_note_beside_its_kept_clean_up_adds_only_what_the_list_lacks() {
        let dir = folder("mo-app-binned-note-beside");
        let bin = holding(Some(vec!["C:/a.dll", "C:/b.dll", "C:/c.dll"]));
        let note = Going::start(&dir, "old-copies", 3);
        for r in [going("C:/a.dll", 3), going("C:/b.dll", 4)] {
            note.went(&r);
        }
        // Kept with only the first (a crash in Windows' administrator step, say, before the second had gone), and the
        // note left for the next look, which adds the second to the same clean-up.
        let (kept, _) =
            record(&dir, &bin, "old-copies", 3, &Done { removed: vec![going("C:/a.dll", 3)], ..Done::default() });
        assert_eq!(kept.list[0].done.removed.len(), 1);
        note.finish(false);
        // Another clean-up, kept since, stays as it is, after it.
        record(&dir, &bin, "old-copies", 10, &Done { removed: vec![going("C:/c.dll", 10)], ..Done::default() });
        let now = refresh(&dir, &bin);
        assert_eq!(whens(&now.list), [3, 10]);
        assert_eq!(paths(&now.list[0]), ["C:/a.dll", "C:/b.dll"]);
        assert!(notes(&dir).is_empty());
        // What the bin doesn't hold (it stayed where it was after all) isn't kept from a note either.
        let note = Going::start(&dir, "old-copies", 20);
        note.went(&going("C:/d.dll", 20));
        drop(note);
        assert_eq!(whens(&refresh(&dir, &bin).list), [3, 10]);
        assert!(notes(&dir).is_empty());
    }

    #[test]
    fn a_note_beside_a_clean_up_kept_a_second_later_goes_with_it() {
        let dir = folder("mo-app-binned-note-later");
        let bin = holding(Some(vec!["C:/old.dll", "C:/a.dll", "C:/b.dll"]));
        // A clean-up at 30 is kept; the clock is put back, and the next one, at 30 too, is kept at 31.
        record(&dir, &bin, "spare-copies", 30, &Done { removed: vec![going("C:/old.dll", 30)], ..Done::default() });
        let note = Going::start(&dir, "spare-copies", 30);
        for r in [going("C:/a.dll", 30), going("C:/b.dll", 30)] {
            note.went(&r);
        }
        // It stopped with an error just as b went: kept with a only, and its note left.
        let (kept, at) =
            record(&dir, &bin, "spare-copies", 30, &Done { removed: vec![going("C:/a.dll", 30)], ..Done::default() });
        assert_eq!((at, whens(&kept.list)), (31, vec![30, 31]));
        note.finish(false);
        // b goes with a, not with the clean-up that was at 30 first.
        let now = refresh(&dir, &bin);
        assert_eq!(whens(&now.list), [30, 31]);
        assert_eq!((paths(&now.list[0]), paths(&now.list[1])), (vec!["C:/old.dll"], vec!["C:/a.dll", "C:/b.dll"]));
    }

    #[test]
    fn a_clean_up_taken_in_from_its_note_while_it_ran_is_kept_once() {
        let dir = folder("mo-app-binned-note-early");
        let bin = holding(Some(vec!["C:/a.dll", "C:/b.dll"]));
        // Another copy of the app took in the first line of its note just as it started (before it was locked).
        let line = NoteLine { kind: "spare-copies".to_owned(), when: 40, removed: going("C:/a.dll", 40) };
        let other = dir.join(format!("{NOTE_START}spare-copies-40-1{NOTE_END}"));
        fs::write(&other, format!("{}\n", serde_json::to_string(&line).unwrap())).unwrap();
        assert_eq!(whens(&refresh(&dir, &bin).list), [40]);
        // Once it ends, it's kept whole, once, as it says it (found in the bin), at its own time.
        let mut found = Done { removed: vec![going("C:/a.dll", 40), going("C:/b.dll", 40)], ..Done::default() };
        for r in &mut found.removed {
            r.bin_id = Some(format!("bin:{}", r.path));
        }
        let (kept, at) = record(&dir, &bin, "spare-copies", 40, &found);
        assert_eq!((at, whens(&kept.list)), (40, vec![40]));
        assert_eq!(kept.list[0].done.removed, found.removed);
    }

    #[test]
    fn a_note_another_copy_of_the_app_is_writing_is_left_be() {
        let dir = folder("mo-app-binned-note-locked");
        let bin = holding(Some(vec!["C:/a.dll"]));
        let note = Going::start(&dir, "spare-copies", 11);
        note.went(&going("C:/a.dll", 11));
        // Locked while its clean-up runs, so another copy of the app can tell.
        let path = notes(&dir).remove(0);
        assert!(matches!(fs::File::open(&path).unwrap().try_lock(), Err(fs::TryLockError::WouldBlock)));
        drop(note);
        // Held as a running clean-up in another copy of the app holds its note.
        let held = open_note(&path).unwrap();
        held.try_lock().unwrap();
        assert!(refresh(&dir, &bin).list.is_empty());
        assert!(path.exists());
        // Nor is it taken away while it's held, however it was taken in.
        remove_note(&path);
        assert!(path.exists());
        // Once it's let go, it's taken in.
        drop(held);
        assert_eq!(whens(&refresh(&dir, &bin).list), [11]);
        assert!(!path.exists());
    }

    #[test]
    fn a_line_a_crash_cut_short_is_left_out() {
        let dir = folder("mo-app-binned-note-cut");
        let line = NoteLine { kind: "spare-copies".to_owned(), when: 12, removed: going("C:/a.dll", 12) };
        let text = format!("{}\n{{\"kind\": \"spare-co", serde_json::to_string(&line).unwrap());
        fs::write(dir.join(format!("{NOTE_START}spare-copies-12-1{NOTE_END}")), text).unwrap();
        // Something else of a name like it, but not a file, isn't one.
        fs::create_dir_all(dir.join(format!("{NOTE_START}spare-copies-13-1{NOTE_END}"))).unwrap();
        let kept = refresh(&dir, &holding(Some(vec!["C:/a.dll"])));
        assert_eq!((whens(&kept.list), paths(&kept.list[0])), (vec![12], vec!["C:/a.dll"]));
        assert_eq!(notes(&dir).len(), 1);
    }

    #[test]
    fn a_note_waits_while_the_list_cant_be_read_or_written() {
        let dir = folder("mo-app-binned-note-waits");
        let bin = holding(Some(vec!["C:/a.dll"]));
        let note = Going::start(&dir, "spare-copies", 14);
        note.went(&going("C:/a.dll", 14));
        drop(note);
        // The list can't be read: the note isn't taken in, so nothing in it is lost.
        fs::create_dir_all(dir.join(FILE)).unwrap();
        assert!(refresh(&dir, &bin).list.is_empty());
        assert_eq!(notes(&dir).len(), 1);
        // It can be read again: taken in, and kept.
        fs::remove_dir(dir.join(FILE)).unwrap();
        let kept = refresh(&dir, &bin);
        assert_eq!((whens(&kept.list), kept.noted), (vec![14], true));
        assert!(notes(&dir).is_empty());
    }

    #[test]
    fn the_plugins_page_hears_of_plugin_files_in_the_bin_before_a_look() {
        let dir = folder("mo-app-binned-plugin-files");
        let back = dir.join("Back.dll");
        fs::write(&back, "put back since").unwrap();
        let back = back.to_string_lossy().into_owned();
        let mut song = going("C:/Songs/old.flp", 1);
        song.what = What::Song(mo_space::song::CompactKind::Backup);
        let mut list = Vec::new();
        add(
            &mut list,
            "spare-copies",
            1,
            &Done { removed: vec![going("C:/a.dll", 1), going(&back, 1)], ..Done::default() },
        );
        add(&mut list, "compact", 2, &Done { removed: vec![song], ..Done::default() });
        write(&dir, &list).unwrap();
        let note = Going::start(&dir, "spare-copies", 3);
        note.went(&going("C:/b.dll", 3));
        // The same one noted again (another copy of the app's note, say) is said once.
        note.went(&going("C:/a.dll", 3));
        drop(note);
        // What's back where it was, and what isn't a plugin's, is left out.
        assert_eq!(plugin_files(&dir), ["C:/a.dll", "C:/b.dll"]);
        // Nothing is changed: the note waits for a look.
        assert_eq!(notes(&dir).len(), 1);
        assert_eq!(read(&dir).unwrap(), list);
    }

    #[test]
    fn each_clean_up_has_a_time_of_its_own() {
        let (a, b) = (now(), now());
        assert!(b > a && a > 1_700_000_000);
    }
}
