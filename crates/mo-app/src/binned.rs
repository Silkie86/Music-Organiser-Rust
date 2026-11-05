//! What the Space page's clean-ups put in the Recycle Bin (the Trash on a Mac or Linux), kept in a small file in the
//! app's folder. Each clean-up can then be put back for as long as it's still in the bin (the Space page's question 5:
//! "every clean-up result has a Put back button"), after a restart too, and the page can say what's still in the bin:
//! the app never empties it itself, and tells you what's there instead (Solomon, 3 Nov 2026: "leave it to me with a
//! warning"). The file only keeps what may still be in the bin (rule 28): each look in the bin drops what it no longer
//! holds, but for what was on a drive that isn't plugged in (its bin can't be looked in until it's back); and of the
//! clean-ups the bin can't vouch for (a Mac's Trash can't be looked in at all), at most [`KEEP`].

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use mo_space::clean::{Bin, Done, Removed};
use mo_space::in_bin::{self, InBin};
use serde::{Deserialize, Serialize};

use crate::app_settings::write_checked;
use crate::log;
use crate::space_bin::{BIN, when_words};

/// The file, in the app's folder.
pub const FILE: &str = "space-binned.json";

/// Of the clean-ups the bin can't vouch for (it can't be looked in, or their drive isn't plugged in), at most this many
/// are kept: the oldest go first. What the bin still holds is always kept.
pub const KEEP: usize = 50;

/// One clean-up: which of the page's it was, when it ran, and what it sent to the bin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binned {
    /// Which clean-up: [`crate::space_spare::SPARE_COPIES`] for spare plugin copies.
    pub kind: String,
    /// When it ran (Unix seconds): with its kind, what Put back knows it by, so no two of a kind share one.
    pub when: i64,
    /// What went, with what the bin calls each: what Put back needs. What couldn't go isn't kept.
    pub done: Done,
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
        Binned { kind: self.kind.clone(), when: self.when, done }
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
/// Gives the time it's kept under: `when`, or a second or more later when another of its kind is kept under that time
/// (the computer's clock was put back since), as Put back knows each one by its kind and time.
pub fn add(list: &mut Vec<Binned>, kind: &str, when: i64, done: &Done) -> i64 {
    let when = free_time(list, kind, when);
    if !done.removed.is_empty() {
        let done = Done { removed: done.removed.clone(), problems: Vec::new(), went_with: done.went_with.clone() };
        list.push(Binned { kind: kind.to_owned(), when, done });
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

/// Reads the clean-ups kept in `folder`, lets `change` change them, keeps what may still be in the bin ([`look`]), and
/// writes them back when that changed them. One at a time, so two changes can't lose each other's. A list that
/// couldn't be written (said in the log) is kept here instead while the app runs: the next change starts from it, not
/// the file, and writes it. A file that couldn't be read isn't written over: what's added meanwhile is kept here, and
/// goes with what it holds once it can be read. Looks in the bin and waits for the disk: call it off the window's
/// thread.
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
    let looked = look(list, bin);
    let noted = file.as_ref() == Some(&looked.keep)
        || write(folder, &looked.keep)
            .map_err(|e| log::warning("Space", &format!("Couldn't keep what the app put in the {BIN} ({FILE}): {e}")))
            .is_ok();
    if !noted {
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
    let mut kept_at = when;
    let kept = update(folder, bin, |list| kept_at = add(list, kind, when, done));
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

    #[test]
    fn each_clean_up_has_a_time_of_its_own() {
        let (a, b) = (now(), now());
        assert!(b > a && a > 1_700_000_000);
    }
}
