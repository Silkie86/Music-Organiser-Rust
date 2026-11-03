//! Rule 28 (Solomon, 2026-11-02: "make sure the app cleans up after itself where possible, I don't want the app filling
//! up my PC as I use it"). Every file the app writes for itself has a limit or goes once it's done with, and this
//! sweep, a little after each start, clears what a crash or a stopped tool left behind.
//!
//! It only removes plain files whose whole name the app gives them, right in the folders it writes them to: never a
//! file by its folder alone, never anything in a folder inside those, never anything with another name, and never
//! through a link: a folder on the way that is a link or a Windows junction is passed over, and so is a file that is
//! one. (The data folder and the Temp folder themselves may be reached through a link, as everything the app writes
//! there is, and `logs\` is followed through one, as below.) What it clears:
//!
//! - The data folder: a settings or database file whose writing a crash cut short (`backup.txt.mo-part`,
//!   `presets.db.new-<number>`), once it's an hour old, so one being written right now is left alone.
//! - `Curves\`: a curve file whose writing a crash cut short (`… .mo-part-<number>-<number>`), an hour old.
//! - `twin\cs\`: the twin test's copy of your library (`library-copy.db`), left when the test was stopped, a day old.
//! - Your Temp folder: the Space page's lists for the administrator step (`mo-admin-<n>-<n>-<n>.txt`), a day old.
//! - `notes\`: F12 notes (`note-<time>.md`) and their pictures (`note-<time>.png`), as both apps save them, about 3
//!   months after they were saved (Solomon, 2026-11-03: "remove older ones"), to the Recycle Bin: see below.
//!
//! `logs\` is kept in bounds at every start instead ([`prune_logs`], from the session log), so an app that crashes soon
//! after it opens still keeps it so: this app's session logs past the newest 30, crash logs (`crash-<time>.log`) past
//! the newest 30, and each tool's reports (the twin test's, the MIDI twin test's, the galaxy check's) past the newest
//! 10 of that kind. The app writes its logs there even when `logs` is a link (the folder moved to another drive), so
//! they're kept in bounds through it too; a file in it that is a link is never one of them.
//!
//! A file's age runs from the latest of when it was made, when it was last changed and, on a Mac or Linux, when its
//! entry in the folder last changed: a copy keeps the changed time of what it was copied from, so a file from 2019
//! copied in today is a day old tomorrow, not seven years old. A file dated up to a day after now (the clock was a
//! little fast when it was written, or has been put back since) waits until its time comes: it isn't removed, and
//! doesn't count among the newest 30 or 10 meanwhile.
//!
//! **F12 notes** are the one thing here you made, so they go to the Recycle Bin rather than straight off the disk, and
//! only when it can take them without asking: Windows asks "permanently delete?" about a file it can't keep (on a
//! network drive or USB stick, or where the Recycle Bin is off or too small), and makes room in a full one by removing
//! the oldest things in it for good; nobody is there to answer, or to lose those. A Mac keeps them for now (its Trash
//! is reached through Finder, which can ask too). The app never empties the Recycle Bin itself (Solomon, 2026-11-03:
//! "leave it to me"; the Space page says what's there). If Windows doesn't move a note (it may have asked all the same,
//! or its Recycle Bin couldn't take it), or says it moved one that's still there, or one is gone though it wasn't
//! moved, or the first file a start sends isn't in the Recycle Bin afterwards, no more notes go by themselves (until
//! the "Stopped" line in [`NOTES_DONE_FILE`] is taken out), and the session log warns of it.
//!
//! A note's age comes from the time in its name, when it was saved, and runs to now or to the newest note, whichever is
//! earlier, so a clock that jumps months on (a flat battery, a date set by hand) makes no note old by itself.
//! [`NOTES_DONE_FILE`] in the data folder says how far the sweep has gone, note by note: it looks at each note once, so
//! a note put back from the Recycle Bin stays, and so does one the Recycle Bin wouldn't take (it's never tried again,
//! and the notes after it wait for the next start, so at most one is tried and fails at each start). A sweep cut off
//! part way (the app closed) goes on from there next time. Two copies of the app never go through the notes at once:
//! the one that does holds a lock on [`NOTES_LOCK_FILE`] (an empty file beside it, which stays), and the other leaves
//! them be. Never the draft (`draft.txt`), the words of a save that failed (`unsaved.txt`) or the lock
//! (`writing.lock`) (rule 17).
//!
//! Never touched: samples (rule 2), projects, "(fixed)" copies (rule 3), converts and exports, F12 notes under 3 months
//! old and the drafts (rule 17), Playground ideas, settings, the databases (library.db, sounds.db and midi.db are the
//! C# app's too, and opened read-only), the C# app's own logs and reports, the launcher's folders (`run-latest-rust\`
//! and its build folders, which it keeps in bounds itself), and everything else in those folders.
//!
//! **A feature that writes temp files later** (a MIDI clip dragged out to a DAW) gives each a name of its own: its own
//! start, the app's process id and a count, such as `mo-clip-<pid>-<n>.mid`, right in the Temp folder (not in a folder
//! of its own: a folder's name proves nothing, since the system or the user can point Temp anywhere, even at the home
//! folder). It removes each one when it's done with it, and adds that exact name here, cleared at the top of the Temp
//! folder only, by its age as above. Never a whole folder by its name or by age alone.

use std::collections::BTreeMap;
use std::fs::{self, Metadata};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{NaiveDateTime, TimeDelta};
use mo_space::clean::{Bin, CANT_LOOK, IN_USE, KEPT_BY_YOU, NOT_IN_BIN, ONLINE_ONLY, Removed, STILL_THERE, What};

use crate::backup::{PART_ENDING, SETTINGS_FILE, SONGS_FILE};
use crate::log::{is_crash_log, is_name_time, is_session_log};
use crate::paths;

/// Crash logs kept: the newest 30, as for the session logs ([`crate::log::KEPT`]).
pub const CRASH_LOGS_KEPT: usize = 30;

/// Reports kept of each kind.
pub const REPORTS_KEPT: usize = 10;

/// F12 notes and their pictures go to the Recycle Bin once they're this old: about 3 months (Solomon, 2026-11-03).
pub const NOTES_KEPT_FOR: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// The file in the data folder that says how far the sweep has gone through the F12 notes: the time (as in a note's
/// name) up to which every note saved has been looked at. One saved before then that's in the folder now was put back,
/// or the Recycle Bin wouldn't take it, so it stays. A line starting [`STOPPED`] says notes no longer go by themselves,
/// and why.
pub const NOTES_DONE_FILE: &str = "tidy-notes.txt";
/// How [`NOTES_DONE_FILE`]'s line saying notes no longer go by themselves starts.
const STOPPED: &str = "Stopped: ";
/// The empty file beside [`NOTES_DONE_FILE`] that a copy of the app holds a lock on while it goes through the F12
/// notes, so that two copies never do at once: one that finds it held leaves the notes to the other. It stays, as it
/// is: were it removed, two copies could each hold a lock, one on the old file and one on a new one.
pub const NOTES_LOCK_FILE: &str = "tidy-notes.lock";
/// How often writing down that no more notes go by themselves is tried, and how long apart: a virus scanner can hold a
/// new file for a moment, and without that line notes go again at the next start.
const STOP_TRIES: usize = 3;
const STOP_RETRY: Duration = Duration::from_millis(200);

/// The tools' reports in `logs\`, by how their names start. Each name is this, then the time it ran (digits, `-` and
/// `_`), then `.txt`: `twin-test-2026-11-02-1430.txt`. Written by `tools/dev/twin-test.ps1`, `tools/dev/MIDI twin
/// test.cmd` and the galaxy check (`tools/galaxycheck`); the C# app writes none of these.
pub const REPORTS: [&str; 6] =
    ["twin-test-", "twin-report-", "twin-songs-", "midi-twin-", "midi-twin-report-", "galaxy-check-"];

/// The folder in the data folder that curve files are written to.
const CURVES: &str = mo_presets::curves::FOLDER;

/// The twin test's copy of the library, beside its C# helper in `twin\cs\` (`tools/twin`, `run_songs`).
const LIBRARY_COPY: &str = "library-copy.db";

/// How the app writes a time into a file's name, in the computer's own time: `2026-11-02_14-30-05`.
const NAME_TIME: &str = "%Y-%m-%d_%H-%M-%S";

/// How long a file being written may take before what's left of it counts as a crash's leftover.
const WRITING: Duration = Duration::from_secs(60 * 60);
pub(crate) const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// How far after now a file's time may be and still count as just made: the clock was a little fast when it was
/// written, or was set back since. Further on than this, the clock was plainly wrong once, and the time counts for
/// nothing (the app's own file is as old as can be; a note of yours stays).
const SKEW: Duration = DAY;
/// A session log changed this recently is never removed to keep the count down: it may be another copy's, still being
/// written. Short, so a crash loop (the app reopened again and again) still keeps the folder near 30.
const LIVE: Duration = Duration::from_secs(10 * 60);

/// How long after start the sweep waits, so it never competes with the first page for the disk (rule 12).
const WAIT: Duration = Duration::from_secs(20);

/// What a sweep did: the app's files it removed, your old F12 notes it put in the Recycle Bin, and what it couldn't.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tidied {
    pub removed: usize,
    pub bytes: u64,
    /// Files of your F12 notes put in the Recycle Bin (not counted in `removed`).
    pub to_bin: usize,
    pub to_bin_bytes: u64,
    /// The app's files that couldn't be removed (in use, or the folder won't let them go): they're tried again next
    /// time.
    pub failed: usize,
    /// Why F12 notes old enough to go stayed, when some did.
    pub notes_stay: Option<String>,
    /// No more F12 notes go to the Recycle Bin by themselves from now on (`notes_stay` says why): said as a warning.
    pub notes_stopped: bool,
}

impl Tidied {
    fn add(&mut self, other: Tidied) {
        self.removed += other.removed;
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.to_bin += other.to_bin;
        self.to_bin_bytes = self.to_bin_bytes.saturating_add(other.to_bin_bytes);
        self.failed += other.failed;
        if self.notes_stay.is_none() {
            self.notes_stay = other.notes_stay;
        }
        self.notes_stopped |= other.notes_stopped;
    }

    /// Removes one file, counting it. In a `shared` folder (the system's Temp, which on a Mac or Linux holds every
    /// account's files), one this account may not remove is someone else's, not a fault.
    fn remove(&mut self, path: &Path, bytes: u64, shared: bool) {
        match fs::remove_file(path) {
            Ok(()) => {
                self.removed += 1;
                self.bytes = self.bytes.saturating_add(bytes);
            }
            // Gone already (another copy of the app swept it first): nothing to do.
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) if e.kind() == ErrorKind::PermissionDenied && shared && !cfg!(windows) => {}
            Err(_) => self.failed += 1,
        }
    }

    /// Puts one of your files in the bin, counting it: whether it went now (`false` when it was gone before it was
    /// sent: you removed it meanwhile, say); says why when it didn't go.
    fn send(&mut self, bin: &dyn Bin, path: &Path, bytes: u64) -> Result<bool, String> {
        if is_gone(path) {
            return Ok(false);
        }
        bin.send(path)?;
        self.to_bin += 1;
        self.to_bin_bytes = self.to_bin_bytes.saturating_add(bytes);
        Ok(true)
    }
}

/// The two folders the sweep looks in: the app's data folder and the system's temp folder. The app sweeps
/// [`Places::this_pc`]; tests make folders of their own.
#[derive(Clone, Debug)]
pub struct Places {
    pub data: PathBuf,
    pub temp: PathBuf,
}

impl Places {
    pub fn this_pc() -> Places {
        Places { data: paths::data_dir(), temp: std::env::temp_dir() }
    }
}

/// Starts the sweep on a low-priority thread of its own, a little after the app opens, and says in the log what it
/// cleared, with what keeping the logs folder in bounds did as the app opened. Never on the window's thread (rules 12
/// and 21).
pub fn start() {
    let started = crate::background::spawn("Tidy up", || {
        std::thread::sleep(WAIT);
        let mut done = sweep(&Places::this_pc(), SystemTime::now(), &mo_space::clean::SystemBin);
        if let Some(pruned) = crate::log::current().and_then(crate::log::SessionLog::take_pruned) {
            done.add(pruned);
        }
        report(&done);
    });
    if let Err(e) = started {
        crate::log::warning("Tidy up", &format!("Couldn't start clearing the app's leftovers: {e}"));
    }
}

fn report(done: &Tidied) {
    let files = |n: usize| if n == 1 { "1 file".to_owned() } else { format!("{n} files") };
    if done.removed > 0 {
        let size = mo_space::size_text(done.bytes);
        crate::log::info("Tidy up", &format!("Cleared {} the app had left behind ({size})", files(done.removed)));
    }
    if done.to_bin > 0 {
        let size = mo_space::size_text(done.to_bin_bytes);
        let what = files(done.to_bin);
        crate::log::info("Tidy up", &format!("Put {what} of F12 notes over 3 months old in the Recycle Bin ({size})"));
    }
    if let Some(why) = &done.notes_stay {
        let line = format!("Some F12 notes over 3 months old stay: {why}");
        if done.notes_stopped { crate::log::warning("Tidy up", &line) } else { crate::log::info("Tidy up", &line) }
    }
    if done.failed > 0 {
        crate::log::warning(
            "Tidy up",
            &format!(
                "{} of the app's leftover files couldn't be cleared (in use?); next time they're tried again",
                done.failed
            ),
        );
    }
}

/// Clears what the app left in `places` (see the top of this file), as it is at `now`, and puts F12 notes over 3
/// months old in `bin` (the computer's Recycle Bin; tests use one of their own). Reads and removes files, so never on
/// the window's thread.
pub fn sweep(places: &Places, now: SystemTime, bin: &dyn Bin) -> Tidied {
    let mut done = Tidied::default();
    done.add(remove_older(&places.data, is_cut_short, WRITING, now, false));
    if let Some(curves) = real_folder(&places.data, &[CURVES]) {
        done.add(remove_older(&curves, mo_presets::export::is_half_written, WRITING, now, false));
    }
    if let Some(twin) = real_folder(&places.data, &["twin", "cs"]) {
        done.add(remove_older(&twin, is_library_copy, DAY, now, false));
    }
    done.add(remove_older(&places.temp, mo_space::admin::is_temp_file, DAY, now, true));
    if let Some(notes) = real_folder(&places.data, &[crate::note::FOLDER]) {
        done.add(old_notes_to_bin(&notes, &places.data.join(NOTES_DONE_FILE), NOTES_KEPT_FOR, now, bin));
    }
    done
}

/// Keeps the logs folder in bounds, as it is at `now` (see the top of this file): this app's session logs past the
/// newest [`crate::log::KEPT`] (never `just_made`, this run's, nor one changed in the last 10 minutes, which may be
/// another copy's, still being written), crash logs past the newest [`CRASH_LOGS_KEPT`], and each tool's reports past
/// the newest [`REPORTS_KEPT`] of that kind. The session log calls it at each start, on a low-priority thread. The
/// folder is listed once, and may be a link: the app writes its logs through it.
pub fn prune_logs(folder: &Path, just_made: Option<&Path>, now: SystemTime) -> Tidied {
    let mut done = Tidied::default();
    // `metadata` follows a link, as writing the logs does; a file in the folder that is a link is never one of them.
    if !fs::metadata(folder).is_ok_and(|m| m.is_dir()) {
        return done;
    }
    let all =
        own_files(folder, |n| is_session_log(n) || is_crash_log(n) || REPORTS.iter().any(|start| is_report(n, start)));
    let of = |is_kind: &dyn Fn(&str) -> bool| -> Vec<(PathBuf, Metadata)> {
        all.iter().filter(|(p, _)| p.file_name().and_then(|n| n.to_str()).is_some_and(is_kind)).cloned().collect()
    };
    done.add(keep_newest(of(&is_session_log), crate::log::KEPT, now, just_made, LIVE));
    done.add(keep_newest(of(&is_crash_log), CRASH_LOGS_KEPT, now, None, Duration::ZERO));
    for start in REPORTS {
        done.add(keep_newest(of(&|n| is_report(n, start)), REPORTS_KEPT, now, None, Duration::ZERO));
    }
    done
}

/// `base` with the folders `inside` it, when each of those really is a folder there: not a link, nor a Windows
/// junction, to somewhere else. `base` itself (the data folder, or the system's temp folder) may be reached through a
/// link of the system's own (`/var` on a Mac), as every file the app writes there is.
fn real_folder(base: &Path, inside: &[&str]) -> Option<PathBuf> {
    let mut at = base.to_path_buf();
    for name in inside {
        at.push(name);
        // `symlink_metadata` looks at the link itself: a link or a junction is not a folder here.
        if !fs::symlink_metadata(&at).is_ok_and(|m| m.is_dir()) {
            return None;
        }
    }
    Some(at)
}

/// Deletes all but the newest `keep` of `files`, newest by the latest of their times (see [`latest`]). A file dated up
/// to [`SKEW`] after `now` neither goes nor counts among the newest until its time comes: the clock was a little fast
/// when it was written, or has been put back since, so it can't be told whether the others are newer. One dated further
/// on (a clock that was plainly wrong once) would hold its place for ever, so it counts as the oldest. `spare`, the
/// file just made, is never deleted and counts as one of the newest, and neither is a file changed less than `live`
/// before `now`, nor a session log another copy of the app is writing now (see [`being_written`]).
fn keep_newest(
    mut files: Vec<(PathBuf, Metadata)>,
    keep: usize,
    now: SystemTime,
    spare: Option<&Path>,
    live: Duration,
) -> Tidied {
    let mut done = Tidied::default();
    let before = files.len();
    if let Some(spare) = spare {
        files.retain(|(p, _)| p != spare);
    }
    let keep = if files.len() < before { keep.saturating_sub(1) } else { keep };
    files.retain(|(_, m)| !latest(m).is_some_and(|t| t > now && t <= now + SKEW));
    if files.len() <= keep {
        return done;
    }
    // Names alone put "…-05-2.log" before "…-05.log", so the time each was written decides.
    files.sort_by_cached_key(|(p, m)| (latest(m).filter(|t| *t <= now), p.clone()));
    for (old, meta) in &files[..files.len() - keep] {
        let is_live = latest(meta).is_some_and(|t| t <= now && now.duration_since(t).unwrap_or_default() < live);
        if !is_live && !being_written(old) {
            done.remove(old, meta.len(), false);
        }
    }
    done
}

/// Mac, Linux: whether a session log is being written by a copy of the app that's still running, however long since
/// its last line: it holds a lock on its log while it writes it (see [`crate::log`]). Asked for a shared lock, which
/// needs only reading (an exclusive one needs writing on a network drive) and is refused all the same while the writer
/// holds its own. On Windows a log being written can't be removed at all, so there's nothing to look for.
#[cfg(unix)]
fn being_written(path: &Path) -> bool {
    fs::File::open(path).is_ok_and(|f| matches!(f.try_lock_shared(), Err(fs::TryLockError::WouldBlock)))
}

#[cfg(not(unix))]
fn being_written(_path: &Path) -> bool {
    false
}

/// Deletes the plain files in `folder` whose names `is_kind` knows, once they're at least `age` old at `now` (see
/// [`age`]). One whose times can't be read stays, and so does one dated up to [`SKEW`] after `now`; one dated further
/// on (the clock was plainly wrong when it was written) is a leftover as old as can be. `shared`: see
/// [`Tidied::remove`].
fn remove_older(
    folder: &Path,
    is_kind: impl Fn(&str) -> bool,
    old_at: Duration,
    now: SystemTime,
    shared: bool,
) -> Tidied {
    let mut done = Tidied::default();
    let old =
        |meta: &Metadata| latest(meta).is_some_and(|t| t > now + SKEW) || age(meta, now).is_some_and(|a| a >= old_at);
    for (path, meta) in own_files(folder, is_kind) {
        // Looked at again just before it goes: the app may have written it anew since the folder was listed (the
        // Backup page saves its settings through the same name).
        if old(&meta) && fs::symlink_metadata(&path).is_ok_and(|again| again.is_file() && old(&again)) {
            done.remove(&path, meta.len(), shared);
        }
    }
    done
}

/// Puts in `bin` the F12 notes saved in `folder` that are `kept_for` old, each note's words (`note-<time>.md`) and
/// picture (`note-<time>.png`) together, and keeps in `done_file` how far it has gone (see the top of this file).
/// Nothing else in the folder is looked at.
fn old_notes_to_bin(folder: &Path, done_file: &Path, kept_for: Duration, now: SystemTime, bin: &dyn Bin) -> Tidied {
    let mut tidied = Tidied::default();
    // Each note by the name its words and picture share, with when it was saved; oldest first.
    let mut notes: BTreeMap<String, (NaiveDateTime, Vec<(PathBuf, Metadata)>)> = BTreeMap::new();
    for (path, meta) in own_files(folder, |n| note_name(n).is_some()) {
        let Some(note) = path.file_name().and_then(|n| n.to_str()).and_then(note_name) else { continue };
        let Some(saved) = saved_at(note) else { continue };
        notes.entry(note.to_owned()).or_insert_with(|| (saved, Vec::new())).1.push((path, meta));
    }
    let (Ok(kept_for), Ok(skew)) = (TimeDelta::from_std(kept_for), TimeDelta::from_std(SKEW)) else { return tidied };
    let now = local_time(now);
    // Old by the newest note too, not only by the clock: a clock that jumps months on makes no note old by itself. A
    // note dated after now (the clock was wrong when it was saved) isn't counted.
    let ahead = now.checked_add_signed(skew).unwrap_or(now);
    let newest = notes.values().map(|(saved, _)| *saved).filter(|saved| *saved <= ahead).max();
    let Some(until) = newest.and_then(|newest| newest.min(now).checked_sub_signed(kept_for)) else { return tidied };
    // No note is old enough: nothing to look up, nor to say.
    if !notes.values().any(|(saved, _)| *saved <= until) {
        return tidied;
    }
    // One copy of the app at a time from here on, until this returns: another going through the notes now leaves them
    // to that one. Everything below that writes `done_file` does so holding this.
    let _held = match hold_notes_lock(&done_file.with_file_name(NOTES_LOCK_FILE)) {
        Ok(held) => held,
        Err(why) => {
            tidied.notes_stay = Some(why);
            return tidied;
        }
    };
    let since = match notes_done(done_file) {
        NotesDone::NotYet => None,
        // Never after now, unless the computer's clock is wrong now (put back), or was when it was written. Which isn't
        // known, so it's left as it is, and no notes go until the clock is past it: started again from an earlier
        // time, a note you put back from the Recycle Bin could go again. (If it was the clock then, notes saved
        // between now and that time count as looked at, and stay.)
        NotesDone::UpTo(since) if since > now => {
            tidied.notes_stay = Some(format!(
                "{NOTES_DONE_FILE} in the data folder says {}, which is after now, so the computer's clock may be \
                 wrong: none go until it's past then",
                since.format(NAME_TIME)
            ));
            return tidied;
        }
        NotesDone::UpTo(since) => Some(since),
        // Not known how far earlier sweeps went, so every note old now counts as looked at: none goes that may have
        // been put back. Notes that grow old from now on go as usual.
        NotesDone::Unreadable => return start_again(done_file, until, "held something else"),
        NotesDone::Stopped(why) => return stopped_already(&why),
        NotesDone::CantLook => return cant_read_done_file(),
    };
    let old: Vec<&(NaiveDateTime, Vec<(PathBuf, Metadata)>)> =
        notes.values().filter(|(saved, _)| *saved <= until && since.is_none_or(|since| *saved > since)).collect();
    if old.is_empty() {
        return tidied;
    }
    // Nobody is there to answer if Windows asks "permanently delete?", and a yes would remove them for good. Asked
    // about them all at once, as the Recycle Bin needs room for them all.
    let bytes = old.iter().flat_map(|(_, parts)| parts).fold(0u64, |sum, (_, meta)| sum.saturating_add(meta.len()));
    if let Some(why) = bin.refuses_quietly(folder, bytes) {
        tidied.notes_stay = Some(why);
        return tidied;
    }
    let mut looked_in_bin = false;
    for (saved, parts) in old {
        // The picture first, so that if it can't go, its words stay with it.
        let mut parts: Vec<&(PathBuf, Metadata)> = parts.iter().collect();
        parts.sort_by_key(|(path, _)| path.extension().is_none_or(|end| end != "png"));
        // Only online in OneDrive, or it couldn't be looked at: the note's own reason, so it stays (looked at, below,
        // so it isn't tried again) and the notes after it go on. Any other reason is the Recycle Bin's (it has filled
        // up since the sweep began, say): this note and the ones after it wait for the next start.
        let own_reason = match parts.iter().find_map(|(path, meta)| bin.refuses_quietly(path, meta.len())) {
            Some(why) if why == ONLINE_ONLY || why == CANT_LOOK => Some(why),
            Some(why) => {
                tidied.notes_stay = Some(why);
                return tidied;
            }
            None => None,
        };
        // Read again just before this note is written down: something else may have changed it since (you, stopping
        // the notes), or got past this one.
        match notes_done(done_file) {
            NotesDone::Stopped(why) => return Tidied { notes_stay: stopped_already(&why).notes_stay, ..tidied },
            NotesDone::UpTo(at) if at > *saved => continue,
            NotesDone::NotYet | NotesDone::UpTo(_) => {}
            NotesDone::Unreadable | NotesDone::CantLook => {
                return Tidied { notes_stay: cant_read_done_file().notes_stay, ..tidied };
            }
        }
        // Written before the note goes, so each is looked at once: one put back from the bin later, or one the bin
        // wouldn't take, stays, and is never tried again. The notes after it aren't counted yet, so if the app closes
        // part way, they go at the next start.
        if let Err(e) = write_notes_done(done_file, *saved, None) {
            return Tidied { notes_stay: cant_write_done_file(&e).notes_stay, ..tidied };
        }
        if let Some(why) = own_reason {
            tidied.notes_stay.get_or_insert(why);
            continue;
        }
        // The Recycle Bin couldn't be read to see the first file is in it: this note's other file goes too, and the
        // notes after it wait for the next start.
        let mut unchecked = false;
        for (path, meta) in parts {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // For the first file to go: where it really is, and when it went, which is how the Recycle Bin is asked
            // for it afterwards.
            let first = (!looked_in_bin).then(|| {
                let shown = path.to_string_lossy().into_owned();
                let real = mo_space::real_path(&shown).unwrap_or_else(|| shown.clone());
                let when = unix_seconds(SystemTime::now());
                Removed { path: shown, real, bytes: meta.len(), what: What::Note, when, bin_id: None }
            });
            match tidied.send(bin, path, meta.len()) {
                // The first file to go is looked for in the bin. Not there, Windows removed it for good (it asks
                // first, and may have been told yes): none go by themselves from now on, not even this note's words.
                Ok(true) if first.is_some() => {
                    looked_in_bin = true;
                    let mut sent: Vec<Removed> = first.into_iter().collect();
                    let problems = bin.find(&mut sent);
                    if problems.iter().any(|p| p.why == NOT_IN_BIN) {
                        // Not counted as put in the Recycle Bin.
                        tidied.to_bin = tidied.to_bin.saturating_sub(1);
                        tidied.to_bin_bytes = tidied.to_bin_bytes.saturating_sub(meta.len());
                        let why = format!(
                            "{name} couldn't be found in the Recycle Bin after it was put there (Windows may have \
                             asked, and removed it for good)"
                        );
                        return stop(tidied, done_file, *saved, &why);
                    }
                    unchecked = !problems.is_empty();
                }
                // It went, or it was gone before its turn (you removed it meanwhile, say).
                Ok(_) => {}
                Err(why) => {
                    // Windows didn't move it (it may have asked first and been told no, or its Recycle Bin couldn't
                    // take it), or said it did but it's still there, or it's gone though it didn't go to the bin (it
                    // may have been removed for good): the Recycle Bin may not take notes quietly here. None go by
                    // themselves from now on.
                    let gone = is_gone(path);
                    if gone || why == KEPT_BY_YOU || why == STILL_THERE {
                        return stop(tidied, done_file, *saved, &stop_reason(&name, &why, gone));
                    }
                    // Otherwise it's the first one the bin won't take, which ends this sweep's notes. It stays, and
                    // isn't tried again; the notes after it wait for the next start, so at most one is tried and
                    // fails at each start.
                    let why = if why == IN_USE { "a program is using it" } else { why.trim_end_matches('.') };
                    tidied.notes_stay = Some(format!(
                        "{name} stays for good: {why}. Put it in the Recycle Bin yourself if you like. The notes \
                         after it wait for the next start"
                    ));
                    return tidied;
                }
            }
        }
        if unchecked {
            tidied.notes_stay = Some(
                "the Recycle Bin couldn't be read to see the first file put there is in it, so the notes after it wait \
                 for the next start"
                    .to_owned(),
            );
            return tidied;
        }
    }
    // Every note saved up to `until` has been looked at now. Unless something else stopped them meanwhile (its
    // "Stopped" line stays), or has written a later time (it stays: going back would let a note you put back go again).
    match notes_done(done_file) {
        NotesDone::Stopped(why) => return Tidied { notes_stay: stopped_already(&why).notes_stay, ..tidied },
        NotesDone::UpTo(at) if at >= until => {}
        // A folder or a link of that name now: left as it is.
        NotesDone::CantLook => {}
        NotesDone::NotYet | NotesDone::UpTo(_) | NotesDone::Unreadable => {
            let _ = write_notes_done(done_file, until, None);
        }
    }
    tidied
}

/// Whether `path` is gone from its folder.
fn is_gone(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err_and(|e| e.kind() == ErrorKind::NotFound)
}

/// Why no more notes go by themselves, said of `name`, the note file Windows didn't put in the Recycle Bin (`why` is
/// what the bin said), or that's `gone` though it didn't go there.
fn stop_reason(name: &str, why: &str, gone: bool) -> String {
    match (gone, why == STILL_THERE) {
        (true, true) => format!(
            "Windows said it put {name} in the Recycle Bin, but it was still in the notes folder after, and is gone now \
             (it may have asked first, and removed it for good)"
        ),
        (true, false) => format!(
            "{name} is gone from the notes folder, but Windows said it couldn't put it in the Recycle Bin (it may have \
             asked first, and removed it for good)"
        ),
        (false, true) => format!(
            "Windows said it put {name} in the Recycle Bin, but it's still in the notes folder (it may have asked \
             first)"
        ),
        (false, false) => format!(
            "Windows didn't put {name} in the Recycle Bin (it may have asked first and been told no, or its Recycle \
             Bin couldn't take it)"
        ),
    }
}

/// Takes the lock on [`NOTES_LOCK_FILE`] at `path` (making it, empty, the first time), held until what's returned is
/// dropped; or says why the notes stay: another copy of the app is going through them now, or the lock can't be had.
/// A link or a folder of that name is never opened. Nothing is ever written in it: it's opened for writing only
/// because some network drives give a lock only on a file opened so.
fn hold_notes_lock(path: &Path) -> Result<fs::File, String> {
    let cant_open = |e: std::io::Error| format!("{NOTES_LOCK_FILE} in the data folder couldn't be opened ({e})");
    let open = |create_new: bool| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(create_new);
        // Windows: while it's held, nobody can remove it (another copy could then make a new one and lock that too).
        #[cfg(windows)]
        std::os::windows::fs::OpenOptionsExt::share_mode(&mut options, 0x1 | 0x2);
        options.open(path)
    };
    // Made new, so nothing there (a link included) is ever opened through; one already there (another copy may have
    // made it a moment ago) is opened only when it's a plain file.
    let file = match open(true) {
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
                return Err(format!(
                    "{NOTES_LOCK_FILE} in the data folder isn't the app's (a folder or a link of that name), so it's \
                     left as it is"
                ));
            }
            open(false).map_err(cant_open)?
        }
        made => made.map_err(cant_open)?,
    };
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => Err("another copy of the app is going through them now".to_owned()),
        Err(fs::TryLockError::Error(e)) => {
            Err(format!("{NOTES_LOCK_FILE} in the data folder couldn't be locked ({e})"))
        }
    }
}

/// Starts [`NOTES_DONE_FILE`] again from `until`, when what it held can't be read: every note old now counts as looked
/// at, and stays. `why` says what was wrong with it.
fn start_again(done_file: &Path, until: NaiveDateTime, why: &str) -> Tidied {
    match write_notes_done(done_file, until, None) {
        Ok(()) => Tidied {
            notes_stay: Some(format!("{NOTES_DONE_FILE} in the data folder {why}, so the notes old now stay")),
            ..Tidied::default()
        },
        Err(e) => cant_write_done_file(&e),
    }
}

/// No more notes go to the Recycle Bin by themselves (`why` says what the Recycle Bin did): [`NOTES_DONE_FILE`] says
/// so from now on, with every note saved up to `up_to` looked at.
fn stop(mut tidied: Tidied, done_file: &Path, up_to: NaiveDateTime, why: &str) -> Tidied {
    tidied.notes_stay = Some(match tried(|| write_notes_done(done_file, up_to, Some(why))) {
        Ok(()) => format!(
            "{why}, so no more go by themselves. To let them go again, take out the line starting \"Stopped:\" in \
             {NOTES_DONE_FILE} in the data folder (don't delete the file: it remembers the notes you put back)"
        ),
        Err(e) => format!(
            "{why}, so no more go by themselves for now; but {NOTES_DONE_FILE} in the data folder couldn't be written \
             to say so ({e}), so they may go again at the next start"
        ),
    });
    tidied.notes_stopped = true;
    tidied
}

/// Runs `write` until it works, at most [`STOP_TRIES`] times, [`STOP_RETRY`] apart; says how the last try went.
fn tried(mut write: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    let mut written = write();
    for _ in 1..STOP_TRIES {
        if written.is_ok() {
            break;
        }
        std::thread::sleep(STOP_RETRY);
        written = write();
    }
    written
}

/// What's said while [`NOTES_DONE_FILE`] says no more notes go by themselves (`why` is its reason).
fn stopped_already(why: &str) -> Tidied {
    let notes_stay = Some(format!("none go by themselves any more ({why}; see {NOTES_DONE_FILE})"));
    Tidied { notes_stay, ..Tidied::default() }
}

fn cant_read_done_file() -> Tidied {
    Tidied { notes_stay: Some(format!("{NOTES_DONE_FILE} in the data folder couldn't be read")), ..Tidied::default() }
}

fn cant_write_done_file(e: &std::io::Error) -> Tidied {
    let notes_stay = Some(format!("{NOTES_DONE_FILE} in the data folder couldn't be written ({e})"));
    Tidied { notes_stay, ..Tidied::default() }
}

/// How far earlier sweeps went through the F12 notes ([`NOTES_DONE_FILE`]).
enum NotesDone {
    /// No sweep has put a note in the bin yet.
    NotYet,
    /// Every note saved up to then has been looked at.
    UpTo(NaiveDateTime),
    /// No more notes go by themselves, and why.
    Stopped(String),
    /// The file holds something else: cut short by a crash, or changed by hand.
    Unreadable,
    /// It couldn't be read, or isn't a plain file (a folder or a link of that name): it's left as it is, and the notes
    /// stay.
    CantLook,
}

fn notes_done(path: &Path) -> NotesDone {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return NotesDone::NotYet,
        Ok(meta) if meta.is_file() => {}
        _ => return NotesDone::CantLook,
    }
    match fs::read_to_string(path) {
        Ok(text) => {
            // An editor may have put a byte-order mark first when you changed it.
            let text = text.trim_start_matches('\u{feff}');
            if let Some(why) = text.lines().find_map(|line| line.strip_prefix(STOPPED)) {
                return NotesDone::Stopped(why.trim().to_owned());
            }
            let first = text.lines().next().unwrap_or_default().trim();
            NaiveDateTime::parse_from_str(first, NAME_TIME).map_or(NotesDone::Unreadable, NotesDone::UpTo)
        }
        Err(e) if e.kind() == ErrorKind::InvalidData => NotesDone::Unreadable,
        Err(_) => NotesDone::CantLook,
    }
}

/// Writes [`NOTES_DONE_FILE`]: every note saved up to `up_to` looked at, and, when `stopped` says why, that no more
/// go by themselves.
fn write_notes_done(path: &Path, up_to: NaiveDateTime, stopped: Option<&str>) -> std::io::Result<()> {
    let mut text = format!(
        "{}\nThe app's clean-up (rule 28) has looked at every F12 note saved up to this time; one still in the notes \
         folder stays. Don't delete this file: it remembers which notes have been looked at, so the ones you put back \
         from the Recycle Bin stay.\n",
        up_to.format(NAME_TIME)
    );
    if let Some(why) = stopped {
        text += &format!(
            "{STOPPED}{why}\nNo more notes go to the Recycle Bin by themselves. To let them go again, take out only the \
             line above that starts \"Stopped:\".\n"
        );
    }
    // Written whole beside it first, then put in its place, so a crash part way leaves the old one whole (with its
    // "Stopped" line, if it has one); the start-up sweep clears what such a crash leaves ([`is_cut_short`]). Made new,
    // so nothing there (a link included) is ever written through.
    let mut part = path.as_os_str().to_owned();
    part.push(PART_ENDING);
    let part = PathBuf::from(part);
    let _ = fs::remove_file(&part);
    let written = fs::OpenOptions::new().write(true).create_new(true).open(&part).and_then(|mut f| {
        f.write_all(text.as_bytes())?;
        // On the disk before it's put in place, so a power cut just after leaves it whole, not empty.
        f.sync_all()
    });
    written.and_then(|()| fs::rename(&part, path)).inspect_err(|_| {
        let _ = fs::remove_file(&part);
    })
}

/// A time as Unix seconds, as the Recycle Bin keeps when things went in.
fn unix_seconds(when: SystemTime) -> i64 {
    when.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// `when` in the computer's own time, as the app writes times into names.
fn local_time(when: SystemTime) -> NaiveDateTime {
    chrono::DateTime::<chrono::Local>::from(when).naive_local()
}

/// When a note was saved, from its name (`note-2026-11-02_14-30-05`, perhaps then `-2`), in the computer's own time.
/// `None` for a time that can't be (the 30th of February).
fn saved_at(note: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(note.strip_prefix("note-")?.get(..19)?, NAME_TIME).ok()
}

/// How old a file is at `now`: from the latest of its times (see [`latest`]). `None` when none can be read, or the
/// latest is after `now`.
fn age(meta: &Metadata, now: SystemTime) -> Option<Duration> {
    now.duration_since(latest(meta)?).ok()
}

/// The latest of when a file was made, when it was last changed and, on a Mac or Linux, when its entry last changed,
/// when any can be read.
fn latest(meta: &Metadata) -> Option<SystemTime> {
    [meta.created().ok(), meta.modified().ok(), entry_changed(meta)].into_iter().flatten().max()
}

/// A Mac or Linux: when the file's entry last changed (it was written, copied in, renamed, or its times were set).
/// Nothing can set this back, so a copy that keeps the times of what it was copied from (a Mac's Finder, `cp -p`)
/// still counts as new.
#[cfg(unix)]
fn entry_changed(meta: &Metadata) -> Option<SystemTime> {
    use std::os::unix::fs::MetadataExt;
    let secs = u64::try_from(meta.ctime()).ok()?;
    let nanos = u32::try_from(meta.ctime_nsec()).ok().filter(|n| *n < 1_000_000_000)?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::new(secs, nanos))
}

/// Windows: when a file was made is enough, as a copy made in Explorer is made anew (one made by a tool that keeps the
/// old times, as robocopy can, counts by them).
#[cfg(not(unix))]
fn entry_changed(_meta: &Metadata) -> Option<SystemTime> {
    None
}

/// The plain files right in `folder` (not in folders inside it) whose names `is_kind` knows, with what the folder's
/// listing says of each. A link is never one, even to a file, and neither is a name that isn't plain text.
fn own_files(folder: &Path, is_kind: impl Fn(&str) -> bool) -> Vec<(PathBuf, Metadata)> {
    let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(&is_kind))
        // The listing's own look (links aren't followed): a link to a file isn't a plain file.
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?)))
        .collect()
}

/// A tool's report named as it names them: `start`, then the time it ran (digits, `-` and `_`, starting with a
/// digit), then `.txt`.
fn is_report(name: &str, start: &str) -> bool {
    let Some(stamp) = name.strip_prefix(start).and_then(|r| r.strip_suffix(".txt")) else { return false };
    stamp.starts_with(|c: char| c.is_ascii_digit())
        && stamp.bytes().all(|b| b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// A file in the data folder whose writing was cut short, named as the code that writes it names it: the Backup
/// settings' (`backup.txt.mo-part`, `backup-songs.txt.mo-part`, see [`crate::backup`]), the sweep's own
/// (`tidy-notes.txt.mo-part`, [`write_notes_done`]), or a side database being made
/// (`presets.db.new-<number>`, `curves.db.new-<number>`, with SQLite's own files beside it; `mo_presets::fixes`).
fn is_cut_short(name: &str) -> bool {
    [SETTINGS_FILE, SONGS_FILE, NOTES_DONE_FILE].iter().any(|f| name.strip_prefix(f) == Some(PART_ENDING))
        || [mo_presets::fixes::FILE_NAME, mo_presets::curves::FILE_NAME]
            .iter()
            .any(|database| mo_presets::fixes::is_half_made(name, database))
}

/// The twin test's copy of the library, or SQLite's own files beside it.
fn is_library_copy(name: &str) -> bool {
    name.strip_prefix(LIBRARY_COPY).is_some_and(|end| ["", "-wal", "-shm", "-journal"].contains(&end))
}

/// A saved F12 note's words or picture, named as both apps name them: `note-`, the time it was saved
/// (`2026-11-02_14-30-05`), perhaps `-` and a number (a second note in the same second), then `.md` or `.png`. Gives
/// the name without its ending, which a note's words and picture share.
fn note_name(name: &str) -> Option<&str> {
    let note = name.strip_suffix(".md").or_else(|| name.strip_suffix(".png"))?;
    is_name_time(note.strip_prefix("note-")?).then_some(note)
}

/// Digits only, at least one.
pub(crate) fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use mo_space::clean::{Problem, Removed};

    use super::*;

    /// A folder for one test, made by the test and removed after: never a real data or temp folder (rule 11).
    fn test_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-tidy-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Writes `path` (and its folders) now.
    fn fresh(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    /// Writes `path` (and its folders), last changed at `when`.
    fn changed_at(path: &Path, when: SystemTime) {
        fresh(path);
        File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
    }

    fn names(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            fs::read_dir(folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    /// A moment in the computer's own time, written as in a note's name (`2026-11-03_12-00-00`).
    fn local(at: &str) -> SystemTime {
        let time = NaiveDateTime::parse_from_str(at, NAME_TIME).unwrap();
        SystemTime::from(time.and_local_timezone(chrono::Local).earliest().unwrap())
    }

    /// A Recycle Bin for tests: a folder of the test's own, never the computer's bin (rule 11).
    struct TestBin(PathBuf);

    impl Bin for TestBin {
        fn send(&self, path: &Path) -> Result<(), String> {
            let name = path.file_name().ok_or("no name")?;
            fs::create_dir_all(&self.0).map_err(|e| e.to_string())?;
            fs::rename(path, self.0.join(name)).map_err(|e| e.to_string())
        }

        fn find(&self, _: &mut [Removed]) -> Vec<Problem> {
            Vec::new()
        }

        fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
    }

    /// A test's bin that can't take some things quietly, as Windows' Recycle Bin can't when it's off or too small.
    struct PickyBin {
        bin: TestBin,
        refuses: fn(&Path, u64) -> Option<String>,
    }

    impl Bin for PickyBin {
        fn send(&self, path: &Path) -> Result<(), String> {
            self.bin.send(path)
        }

        fn find(&self, _: &mut [Removed]) -> Vec<Problem> {
            Vec::new()
        }

        fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }

        fn refuses_quietly(&self, path: &Path, bytes: u64) -> Option<String> {
            (self.refuses)(path, bytes)
        }
    }

    /// A test's bin that won't take pictures, as when a program has one open (or the answer to Windows was no).
    struct NoPictures(TestBin);

    impl Bin for NoPictures {
        fn send(&self, path: &Path) -> Result<(), String> {
            if path.extension().is_some_and(|end| end == "png") {
                return Err("a program is using it".to_owned());
            }
            self.0.send(path)
        }

        fn find(&self, _: &mut [Removed]) -> Vec<Problem> {
            Vec::new()
        }

        fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
    }

    /// A test's bin that acts as Windows' Recycle Bin can when it can't keep something: it asks first (`asks`: and is
    /// told no), or takes it but afterwards hasn't got it (it was told yes, and removed it for good), or can't be read
    /// to see what it has.
    struct OddBin {
        bin: TestBin,
        odd: Odd,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Odd {
        Asks,
        Loses,
        CantBeRead,
        /// Says Windows didn't move it, but it's gone (removed for good after all).
        GoneButKept,
        /// Says Windows moved it, but it's still there.
        StillThere,
        /// Says Windows moved it but it was still there, and it's gone a moment later (removed for good while a
        /// program had it open).
        Lingers,
    }

    impl Bin for OddBin {
        fn send(&self, path: &Path) -> Result<(), String> {
            match self.odd {
                Odd::Asks => Err(KEPT_BY_YOU.to_owned()),
                Odd::GoneButKept => {
                    fs::remove_file(path).map_err(|e| e.to_string())?;
                    Err(KEPT_BY_YOU.to_owned())
                }
                Odd::StillThere => Err(STILL_THERE.to_owned()),
                Odd::Lingers => {
                    fs::remove_file(path).map_err(|e| e.to_string())?;
                    Err(STILL_THERE.to_owned())
                }
                _ => self.bin.send(path),
            }
        }

        fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
            let why = match self.odd {
                Odd::Loses => NOT_IN_BIN,
                _ => "The Recycle Bin couldn't be read to check it's there.",
            };
            removed.iter().map(|r| Problem { path: r.path.clone(), why: why.to_owned() }).collect()
        }

        fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
    }

    /// A test's bin that keeps the order files were sent in, and counts how often it was looked in.
    struct WatchedBin {
        bin: TestBin,
        sent: std::cell::RefCell<Vec<String>>,
        looked: std::cell::Cell<usize>,
    }

    impl Bin for WatchedBin {
        fn send(&self, path: &Path) -> Result<(), String> {
            self.sent.borrow_mut().push(path.file_name().unwrap().to_string_lossy().into_owned());
            self.bin.send(path)
        }

        fn find(&self, _: &mut [Removed]) -> Vec<Problem> {
            self.looked.set(self.looked.get() + 1);
            Vec::new()
        }

        fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
    }

    /// A test's bin with another copy of the app beside it, which stops notes going by themselves while this one is
    /// sending its first (its "Stopped" line written into what says how far the sweep went).
    struct OtherCopyStops {
        bin: TestBin,
        done_file: PathBuf,
    }

    impl Bin for OtherCopyStops {
        fn send(&self, path: &Path) -> Result<(), String> {
            let mut text = fs::read_to_string(&self.done_file).map_err(|e| e.to_string())?;
            if !text.contains(STOPPED) {
                text += &format!("{STOPPED}the other copy's reason\n");
                fs::write(&self.done_file, text).map_err(|e| e.to_string())?;
            }
            self.bin.send(path)
        }

        fn find(&self, _: &mut [Removed]) -> Vec<Problem> {
            Vec::new()
        }

        fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
    }

    /// A test's bin with another copy of the app beside it that has got further: while this one sends its first note,
    /// the other says it has looked at every note saved up to `ahead_to`.
    struct OtherCopyAhead {
        bin: TestBin,
        done_file: PathBuf,
        ahead_to: &'static str,
        said: std::cell::Cell<bool>,
    }

    impl Bin for OtherCopyAhead {
        fn send(&self, path: &Path) -> Result<(), String> {
            if !self.said.replace(true) {
                fs::write(&self.done_file, format!("{}\n", self.ahead_to)).map_err(|e| e.to_string())?;
            }
            self.bin.send(path)
        }

        fn find(&self, _: &mut [Removed]) -> Vec<Problem> {
            Vec::new()
        }

        fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
    }

    /// A test's bin with something else beside it (you, in an editor) that stops notes going by themselves while this
    /// copy is looking at whether the Recycle Bin can take its first note.
    struct StopsWhileLooking {
        bin: TestBin,
        done_file: PathBuf,
        folder: PathBuf,
    }

    impl Bin for StopsWhileLooking {
        fn send(&self, path: &Path) -> Result<(), String> {
            self.bin.send(path)
        }

        fn find(&self, _: &mut [Removed]) -> Vec<Problem> {
            Vec::new()
        }

        fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }

        fn refuses_quietly(&self, path: &Path, _bytes: u64) -> Option<String> {
            // Asked about a note, not the folder: the "Stopped" line goes in, once.
            let text = fs::read_to_string(&self.done_file).unwrap_or_default();
            if path != self.folder && !text.contains(STOPPED) {
                let first = text.lines().next().unwrap_or("2026-06-01_10-00-00").to_owned();
                fs::write(&self.done_file, format!("{first}\n{STOPPED}your reason\n")).unwrap();
            }
            None
        }
    }

    /// A test's bin that Windows asks about the first file it's sent (and is told no), after which what says how far
    /// the sweep went can't be written: a folder is where it's written first.
    struct AsksAndCantBeWritten {
        part: PathBuf,
    }

    impl Bin for AsksAndCantBeWritten {
        fn send(&self, _: &Path) -> Result<(), String> {
            fs::create_dir_all(self.part.join("in the way")).map_err(|e| e.to_string())?;
            Err(KEPT_BY_YOU.to_owned())
        }

        fn find(&self, _: &mut [Removed]) -> Vec<Problem> {
            Vec::new()
        }

        fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
    }

    /// A test's bin the app closes in front of, at the second file it's sent (the test stops there).
    struct ClosingBin {
        bin: TestBin,
        sent: std::cell::Cell<usize>,
    }

    impl Bin for ClosingBin {
        fn send(&self, path: &Path) -> Result<(), String> {
            self.sent.set(self.sent.get() + 1);
            assert!(self.sent.get() < 2, "the app closed");
            self.bin.send(path)
        }

        fn find(&self, _: &mut [Removed]) -> Vec<Problem> {
            Vec::new()
        }

        fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
    }

    /// A test's bin that says each file couldn't go, though it's gone afterwards (Windows may have removed it for good).
    struct GoneAfterAnError;

    impl Bin for GoneAfterAnError {
        fn send(&self, path: &Path) -> Result<(), String> {
            fs::remove_file(path).map_err(|e| e.to_string())?;
            Err("It couldn't go to the Recycle Bin (Os { code: -2147024891 }).".to_owned())
        }

        fn find(&self, _: &mut [Removed]) -> Vec<Problem> {
            Vec::new()
        }

        fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
    }

    /// A test's bin beside which you remove the note after the one being sent, as the sweep goes (in Explorer, say).
    struct YouRemoveTheNext {
        bin: TestBin,
        next: PathBuf,
    }

    impl Bin for YouRemoveTheNext {
        fn send(&self, path: &Path) -> Result<(), String> {
            let _ = fs::remove_file(&self.next);
            self.bin.send(path)
        }

        fn find(&self, _: &mut [Removed]) -> Vec<Problem> {
            Vec::new()
        }

        fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
    }

    /// A test's bin that looks, as each file is sent, whether the lock on going through the notes is held then:
    /// another copy of the app trying it would be refused.
    struct ChecksTheLock {
        bin: TestBin,
        lock: PathBuf,
        held: std::cell::Cell<usize>,
    }

    impl Bin for ChecksTheLock {
        fn send(&self, path: &Path) -> Result<(), String> {
            let other = File::open(&self.lock).map_err(|e| e.to_string())?;
            if matches!(other.try_lock(), Err(fs::TryLockError::WouldBlock)) {
                self.held.set(self.held.get() + 1);
            }
            self.bin.send(path)
        }

        fn find(&self, _: &mut [Removed]) -> Vec<Problem> {
            Vec::new()
        }

        fn put_back(&self, _: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
    }

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn crash_logs_and_reports_keep_their_newest_and_nothing_else_goes() {
        let dir = test_folder("logs");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let logs = places.data.join("logs");
        let now = SystemTime::now();
        // Made oldest first: a log's age runs from the later of when it was made and changed.
        for i in 0..35u32 {
            changed_at(&logs.join(format!("crash-{}.log", 1_700_000_000 + i)), now - MINUTE * (100 - i));
        }
        // Dated after now: it counts as the oldest, rather than holding a place for ever.
        changed_at(&logs.join("crash-4000000000.log"), now + DAY * 3650);
        for i in 0..12u32 {
            changed_at(&logs.join(format!("twin-report-2026-10-{:02}-1200.txt", i + 1)), now - MINUTE * (50 - i));
            changed_at(&logs.join(format!("midi-twin-2026-10-{:02}-1200.txt", i + 1)), now - MINUTE * (50 - i));
        }
        // Not the app's, or not named as the app names them: all stay, however many and however old.
        let theirs = [
            "session-2000-01-01_00-00-00.log",
            "audio.log",
            "openal.log",
            "crash-report.log",
            "crash-1.log.txt",
            "twin-report-notes.txt",
            "twin-report-2026.txt.bak",
            "my twin-test-2026-10-01-1200.txt",
            "rust-session-notes.txt",
        ];
        for name in theirs {
            changed_at(&logs.join(name), now - DAY * 400);
        }
        // A folder named like a crash log is never removed.
        fs::create_dir_all(logs.join("crash-1600000000.log")).unwrap();

        // The sweep leaves the logs folder to the start-up pruning.
        assert_eq!(sweep(&places, now + DAY * 4000, &bin), Tidied::default());
        let done = prune_logs(&logs, None, now + DAY);
        assert_eq!(done.removed, 6 + 2 + 2);
        assert_eq!(done.failed, 0);
        let left = names(&logs);
        assert_eq!(left.iter().filter(|n| is_crash_log(n)).count(), CRASH_LOGS_KEPT + 1, "30 files and the folder");
        assert!(
            left.contains(&"crash-1700000034.log".to_owned()) && !left.contains(&"crash-1700000004.log".to_owned())
        );
        assert!(!left.contains(&"crash-4000000000.log".to_owned()), "the one dated after now went first");
        assert_eq!(left.iter().filter(|n| is_report(n, "twin-report-")).count(), REPORTS_KEPT);
        assert_eq!(left.iter().filter(|n| is_report(n, "midi-twin-")).count(), REPORTS_KEPT);
        assert!(!left.contains(&"twin-report-2026-10-01-1200.txt".to_owned()), "the oldest goes");
        for name in theirs {
            assert!(left.contains(&name.to_owned()), "{name} stays");
        }
        assert!(!bin.0.exists(), "nothing of the logs goes to the bin");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_tools_reports_keep_their_newest_10() {
        let dir = test_folder("reports");
        let logs = dir.join("logs");
        let now = SystemTime::now();
        let name = |start: &str, day: u32| format!("{start}2026-10-{day:02}-1200.txt");
        for start in REPORTS {
            for day in 1..=12 {
                changed_at(&logs.join(name(start, day)), now - MINUTE * (50 - day));
            }
        }
        // A minute on: each file was made a moment after `now`, so until then it counts as dated ahead.
        let done = prune_logs(&logs, None, now + MINUTE);
        assert_eq!(done.removed, 2 * REPORTS.len());
        for start in REPORTS {
            assert!(!logs.join(name(start, 2)).exists() && logs.join(name(start, 3)).exists(), "{start}");
        }
        assert_eq!(names(&logs).len(), REPORTS_KEPT * REPORTS.len());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_logs_keep_their_newest_30_and_any_still_being_written() {
        let dir = test_folder("sessions");
        let logs = dir.join("logs");
        let name = |i: u32| format!("rust-session-2026-10-01_12-00-{i:02}.log");
        // Made oldest first, all a moment ago (a crash loop, or other copies of the app starting).
        for i in 0..35 {
            fresh(&logs.join(name(i)));
        }
        // And one dated 10 years on: the clock was plainly wrong when it was written.
        let ahead = logs.join("rust-session-2036-10-01_12-00-00.log");
        changed_at(&ahead, SystemTime::now() + DAY * 3650);
        let now = SystemTime::now();
        // Each was written in the last 10 minutes, so it may be another copy's, still being written: none goes yet but
        // the one dated years ahead, which counts as the oldest.
        let done = prune_logs(&logs, None, now + MINUTE * 5);
        assert_eq!(done, Tidied { removed: 1, bytes: 1, ..Tidied::default() });
        assert!(!ahead.exists(), "it never counts as being written now");
        // Later on, only the newest 30 stay, and the log just made always.
        let done = prune_logs(&logs, Some(&logs.join(name(0))), now + MINUTE * 11);
        assert_eq!(done.removed, 5);
        assert_eq!(names(&logs).len(), crate::log::KEPT);
        assert!(logs.join(name(0)).exists(), "the log just made stays, oldest or not");
        assert!((1..=5).all(|i| !logs.join(name(i)).exists()) && logs.join(name(6)).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn logs_dated_a_little_ahead_wait_and_the_newest_others_stay() {
        let dir = test_folder("ahead-logs");
        let logs = dir.join("logs");
        let name = |i: u32| format!("rust-session-2026-10-01_12-00-{i:02}.log");
        let later = SystemTime::now() + DAY;
        // Written while the clock was an hour fast, which has been put right since.
        for i in 1..=30 {
            changed_at(&logs.join(name(i)), later + MINUTE * 60);
        }
        // The last run's log, written since, and an old one.
        changed_at(&logs.join(name(31)), later - MINUTE * 20);
        changed_at(&logs.join(name(0)), later - DAY * 3);
        // Until their time comes, the logs dated ahead neither go nor count: the last run's log stays.
        assert_eq!(prune_logs(&logs, None, later), Tidied::default());
        assert!(logs.join(name(31)).exists() && logs.join(name(0)).exists());
        // Then they count as usual.
        let done = prune_logs(&logs, None, later + MINUTE * 120);
        assert_eq!(done.removed, 2);
        assert!(!logs.join(name(0)).exists() && !logs.join(name(31)).exists() && logs.join(name(1)).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Mac, Linux: a session log another copy of the app is still writing (it holds a lock on it) never goes, however
    /// long since its last line.
    #[cfg(unix)]
    #[test]
    fn a_log_another_copy_is_writing_stays() {
        let dir = test_folder("written-logs");
        let logs = dir.join("logs");
        let name = |i: u32| format!("rust-session-2026-10-01_12-00-{i:02}.log");
        let now = SystemTime::now();
        for i in 0..=30 {
            changed_at(&logs.join(name(i)), now - DAY + MINUTE * i);
        }
        // Past the 10 minutes in which any log may be another copy's: the oldest would go, but its copy holds a lock.
        let later = now + MINUTE * 11;
        let writing = File::open(logs.join(name(0))).unwrap();
        writing.try_lock().unwrap();
        assert_eq!(prune_logs(&logs, None, later), Tidied::default());
        assert!(logs.join(name(0)).exists());
        drop(writing);
        assert_eq!(prune_logs(&logs, None, later).removed, 1);
        assert!(!logs.join(name(0)).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Mac, Linux: a leftover of the app's that can't be removed is counted as such, except in the system's Temp
    /// folder, which holds every account's files.
    #[cfg(unix)]
    #[test]
    fn a_leftover_that_cant_be_removed_is_counted() {
        use std::os::unix::fs::PermissionsExt;
        let dir = test_folder("locked");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let part = places.data.join("backup.txt.mo-part");
        let list = places.temp.join("mo-admin-1-2-3.txt");
        fresh(&part);
        fresh(&list);
        let made = SystemTime::now();
        let shut = |folder: &Path, mode: u32| fs::set_permissions(folder, fs::Permissions::from_mode(mode)).unwrap();
        shut(&places.data, 0o555);
        shut(&places.temp, 0o555);
        // An account that may remove anything (root) can't be shown this.
        let may_remove_anything = fs::remove_file(&part).is_ok();
        // Two days on, both are old enough to go.
        let done = (!may_remove_anything).then(|| sweep(&places, made + DAY * 2, &bin));
        let both_stay = part.exists() && list.exists();
        // Opened again before anything is checked, so a failed check still leaves folders that can be removed.
        shut(&places.data, 0o755);
        shut(&places.temp, 0o755);
        if let Some(done) = done {
            assert_eq!(done, Tidied { failed: 1, ..Tidied::default() });
            assert!(both_stay);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn leftovers_dated_far_on_go_and_ones_a_little_ahead_wait() {
        let dir = test_folder("ahead");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let now = SystemTime::now();
        // Written when the clock was plainly wrong: a leftover as old as can be.
        let far = places.temp.join("mo-admin-1-1-1.txt");
        changed_at(&far, now + DAY * 3);
        // Written a moment ago on a PC whose clock is a little fast: still in use, perhaps.
        let near = places.temp.join("mo-admin-2-2-2.txt");
        changed_at(&near, now + MINUTE * 30);
        let near_part = places.data.join("backup.txt.mo-part");
        changed_at(&near_part, now + MINUTE * 30);
        assert_eq!(sweep(&places, now, &bin), Tidied { removed: 1, bytes: 1, ..Tidied::default() });
        assert!(!far.exists() && near.exists() && near_part.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn leftovers_go_once_old_enough_and_the_users_files_never() {
        let dir = test_folder("leftovers");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let data = &places.data;
        // Gone an hour after they were written: what's left of a write.
        let writes = [
            data.join("backup.txt.mo-part"),
            data.join("backup-songs.txt.mo-part"),
            data.join("presets.db.new-4120"),
            data.join("curves.db.new-77-journal"),
            data.join("Curves").join("Rise abcd1234.bwcurve.mo-part-4120-3"),
            data.join("tidy-notes.txt.mo-part"),
        ];
        // Gone a day after: they may be in use for a while.
        let days = [
            data.join("twin").join("cs").join("library-copy.db"),
            data.join("twin").join("cs").join("library-copy.db-wal"),
            places.temp.join("mo-admin-4120-1730000000000000000-0.txt"),
            places.temp.join("mo-admin-4120-1730000000000000000-0-result.txt"),
        ];
        // The user's and the C# app's: every one stays, however old.
        let kept = [
            data.join("library.db"),
            data.join("library.db-wal"),
            data.join("sounds.db"),
            data.join("midi.db"),
            data.join("presets.db"),
            data.join("curves.db"),
            data.join("midi-index.db"),
            data.join("backup.txt"),
            data.join("backup-songs.txt"),
            data.join("tidy-notes.txt"),
            data.join("tidy-notes.lock"),
            data.join("presets.db.new-"),
            data.join("presets.db.new-12.bak"),
            data.join("notes").join("draft.txt"),
            data.join("notes").join("unsaved.txt"),
            data.join("notes").join("writing.lock"),
            data.join("notes").join("backup.txt.mo-part"),
            data.join("playground").join("current.json"),
            data.join("Bitwig template").join("Music Organiser template.bwproject"),
            data.join("Curves").join("Rise abcd1234.bwcurve"),
            data.join("Curves").join(".mo-part-1-2"),
            data.join("twin").join("cs").join("twin-cs.dll"),
            data.join("twin").join("cs").join("library-copy.db.bak"),
            data.join("library-copy.db"),
            places.temp.join("mo-move-0123abcd.txt"),
            places.temp.join("mo-admin-1-2.txt"),
            places.temp.join("someone else's.mid"),
            places.temp.join("presets.db.new-12"),
            // Notes are looked for only in the notes folder: this one has a newer one beside it, so it would go if
            // Temp were looked in too.
            places.temp.join("note-2026-01-05_10-00-00.md"),
            places.temp.join("note-2026-10-30_09-00-00.md"),
            places.temp.join("MusicOrganiser-Rust").join("Bass line.mid"),
        ];
        for path in writes.iter().chain(&days).chain(&kept) {
            fresh(path);
        }
        // Ages run from when each was made, so the sweep is run as if later on.
        let made = SystemTime::now();
        assert_eq!(sweep(&places, made + MINUTE * 10, &bin), Tidied::default(), "all too new");
        let done = sweep(&places, made + MINUTE * 61, &bin);
        assert_eq!(done, Tidied { removed: writes.len(), bytes: writes.len() as u64, ..Tidied::default() });
        assert!(writes.iter().all(|p| !p.exists()) && days.iter().all(|p| p.exists()), "only the hour's go");
        let done = sweep(&places, made + DAY + MINUTE, &bin);
        assert_eq!(done, Tidied { removed: days.len(), bytes: days.len() as u64, ..Tidied::default() });
        for path in writes.iter().chain(&days) {
            assert!(!path.exists(), "{} goes", path.display());
        }
        assert_eq!(sweep(&places, made + DAY * 4000, &bin), Tidied::default());
        for path in &kept {
            assert!(path.exists(), "{} stays", path.display());
        }
        assert!(!bin.0.exists(), "the app's leftovers go for good, not to the bin");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn notes_go_to_the_bin_3_months_after_they_were_saved_and_drafts_never() {
        let dir = test_folder("notes");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let notes = places.data.join("notes");
        // Saved notes, named as both apps name them: words, and a picture beside them. The newest was saved on the
        // 30th of October at 9:00, so those saved by the 1st of August at 9:00 are 3 months older.
        let old = [
            notes.join("note-2026-07-01_10-00-00.md"),
            notes.join("note-2026-07-01_10-00-00.png"),
            notes.join("note-2026-07-01_10-00-00-2.md"),
            notes.join("note-2026-07-20_23-59-59.png"),
            notes.join("note-2026-08-01_09-00-00.md"),
        ];
        let young = [
            notes.join("note-2026-08-01_09-00-01.md"),
            notes.join("note-2026-10-30_09-00-00.md"),
            notes.join("note-2026-10-30_09-00-00.png"),
            // Dated after now (the clock was wrong when it was saved): it stays, and isn't counted as the newest.
            notes.join("note-2027-03-01_08-00-00.md"),
        ];
        // Never: the draft, the words of a save that failed, the lock, and anything named otherwise or elsewhere.
        let kept = [
            notes.join("draft.txt"),
            notes.join("unsaved.txt"),
            notes.join("writing.lock"),
            notes.join("note-2026-07-01_10-00-00 - Copy.md"),
            notes.join("note-2026-07-01_10-00-00.md.bak"),
            notes.join("note-2026-07-01.md"),
            notes.join("note-2026-07-01_10-00-00.txt"),
            notes.join("note-2026-07-01_10-00-00.md.png"),
            notes.join("my note-2026-07-01_10-00-00.md"),
            // Windows and a Mac take a name in other capitals as the same file, so this one has a time of its own.
            notes.join("Note-2026-07-02_10-00-00.md"),
            // A day there isn't.
            notes.join("note-2026-02-30_10-00-00.md"),
            // Each with a newer note beside it, so it would be old enough if its folder were looked in.
            notes.join("old").join("note-2026-07-01_10-00-00.md"),
            notes.join("old").join("note-2026-10-30_09-00-00.md"),
            places.data.join("note-2026-07-01_10-00-00.md"),
            places.data.join("note-2026-10-30_09-00-00.md"),
            places.data.join("logs").join("note-2026-07-01_10-00-00.md"),
            places.data.join("logs").join("note-2026-10-30_09-00-00.md"),
        ];
        // How old a file looks on the disk doesn't count: the time in a note's name does.
        for path in old.iter().chain(&young).chain(&kept) {
            changed_at(path, local("2001-01-01_12-00-00"));
        }
        // A folder named like a note is never sent.
        fs::create_dir_all(notes.join("note-2026-06-01_00-00-00.md")).unwrap();
        let done_file = places.data.join(NOTES_DONE_FILE);

        assert_eq!(sweep(&places, local("2026-09-15_12-00-00"), &bin), Tidied::default(), "not 3 months old yet");
        assert!(!done_file.exists(), "written only once a note is old enough");
        let now = local("2026-11-03_12-00-00");
        let done = sweep(&places, now, &bin);
        assert_eq!(done, Tidied { to_bin: old.len(), to_bin_bytes: old.len() as u64, ..Tidied::default() });
        for path in &old {
            let name = path.file_name().unwrap();
            assert!(!path.exists() && bin.0.join(name).exists(), "{} is in the bin", path.display());
        }
        for path in young.iter().chain(&kept) {
            assert!(path.exists(), "{} stays", path.display());
        }
        assert!(notes.join("note-2026-06-01_00-00-00.md").is_dir());
        assert_eq!(fs::read_to_string(&done_file).unwrap().lines().next(), Some("2026-08-01_09-00-00"));
        assert_eq!(sweep(&places, now, &bin), Tidied::default(), "nothing more");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_put_back_from_the_bin_stays() {
        let dir = test_folder("put-back");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let notes = places.data.join("notes");
        let note = notes.join("note-2026-07-01_10-00-00.md");
        fresh(&note);
        fresh(&notes.join("note-2026-10-30_09-00-00.md"));
        let now = local("2026-11-03_12-00-00");
        assert_eq!(sweep(&places, now, &bin).to_bin, 1);
        // Put back from the bin, with the same name and times as before.
        fs::rename(bin.0.join("note-2026-07-01_10-00-00.md"), &note).unwrap();
        assert_eq!(sweep(&places, now, &bin), Tidied::default());
        assert_eq!(sweep(&places, local("2027-06-01_12-00-00"), &bin), Tidied::default());
        assert!(note.exists(), "it stays");
        // Notes that grow old later still go.
        fresh(&notes.join("note-2026-09-01_10-00-00.md"));
        fresh(&notes.join("note-2026-12-20_10-00-00.md"));
        assert_eq!(sweep(&places, local("2027-01-01_12-00-00"), &bin).to_bin, 1);
        assert!(!notes.join("note-2026-09-01_10-00-00.md").exists() && note.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clock_months_ahead_makes_no_note_old_by_itself() {
        let dir = test_folder("clock");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let notes = places.data.join("notes");
        let saved = [notes.join("note-2026-09-01_10-00-00.md"), notes.join("note-2026-10-30_09-00-00.md")];
        for path in &saved {
            fresh(path);
        }
        // A year on by the clock, but the newest note is from October: those before it in September aren't 3 months
        // older than it.
        assert_eq!(sweep(&places, local("2027-11-03_12-00-00"), &bin), Tidied::default());
        assert!(saved.iter().all(|p| p.exists()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_notes_stay_while_the_bin_cant_take_them_quietly() {
        let dir = test_folder("picky");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let words = notes.join("note-2026-07-01_10-00-00.md");
        let picture = notes.join("note-2026-07-01_10-00-00.png");
        let other = notes.join("note-2026-07-02_10-00-00.md");
        fresh(&words);
        fs::write(&picture, b"xy").unwrap();
        fresh(&other);
        fresh(&notes.join("note-2026-10-30_09-00-00.md"));
        let now = local("2026-11-03_12-00-00");
        let done_file = places.data.join(NOTES_DONE_FILE);

        // The Recycle Bin is off: nothing goes, and nothing is written down, so they go once it's on again.
        let off = PickyBin { bin: TestBin(dir.join("bin")), refuses: |_, _| Some("it's off".to_owned()) };
        let done = sweep(&places, now, &off);
        assert_eq!(done, Tidied { notes_stay: Some("it's off".to_owned()), ..Tidied::default() });
        assert!(words.exists() && picture.exists() && other.exists() && !done_file.exists());

        // A Recycle Bin without room for all of them at once (4 bytes): none go, and nothing is written down.
        let full =
            PickyBin { bin: TestBin(dir.join("bin")), refuses: |_, bytes| (bytes >= 4).then(|| "it's full".into()) };
        let done = sweep(&places, now, &full);
        assert_eq!(done, Tidied { notes_stay: Some("it's full".to_owned()), ..Tidied::default() });
        assert!(words.exists() && picture.exists() && other.exists() && !done_file.exists());

        // The Recycle Bin filled up since the sweep began: the note it can't take now isn't looked at yet, and neither
        // are the ones after it, so they go at the next start.
        let filled = PickyBin {
            bin: TestBin(dir.join("bin")),
            refuses: |path, _| path.extension().is_some_and(|end| end == "png").then(|| "it's full now".into()),
        };
        let done = sweep(&places, now, &filled);
        assert_eq!(done, Tidied { notes_stay: Some("it's full now".to_owned()), ..Tidied::default() });
        assert!(words.exists() && picture.exists() && other.exists() && !done_file.exists());

        // Only online in OneDrive, and then the Recycle Bin fills up: the first note is looked at (and stays), and the
        // second waits for the next start.
        let online_then_full = PickyBin {
            bin: TestBin(dir.join("bin")),
            refuses: |path, _| {
                let name = path.file_name()?.to_str()?;
                if name.ends_with(".png") {
                    Some(ONLINE_ONLY.into())
                } else {
                    name.starts_with("note-2026-07-02").then(|| "it's full now".into())
                }
            },
        };
        let done = sweep(&places, now, &online_then_full);
        assert_eq!(done, Tidied { notes_stay: Some("it's full now".to_owned()), ..Tidied::default() });
        assert!(words.exists() && picture.exists() && other.exists());
        assert_eq!(how_far(&done_file).0, "2026-07-01_10-00-00");
        fs::remove_file(&done_file).unwrap();

        // A picture that's only online in OneDrive: that note stays whole, for good; the other goes.
        let online = PickyBin {
            bin: TestBin(dir.join("bin")),
            refuses: |path, _| path.extension().is_some_and(|end| end == "png").then(|| ONLINE_ONLY.into()),
        };
        let done = sweep(&places, now, &online);
        assert_eq!(
            done,
            Tidied { to_bin: 1, to_bin_bytes: 1, notes_stay: Some(ONLINE_ONLY.to_owned()), ..Tidied::default() }
        );
        assert!(words.exists() && picture.exists() && !other.exists());
        // It was looked at once, so it isn't tried again.
        assert_eq!(sweep(&places, now, &TestBin(dir.join("bin"))), Tidied::default());
        assert!(words.exists() && picture.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_the_bin_wont_take_stays_and_the_rest_wait_for_the_next_start() {
        let dir = test_folder("stubborn");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let words = notes.join("note-2026-07-01_10-00-00.md");
        let picture = notes.join("note-2026-07-01_10-00-00.png");
        let next = notes.join("note-2026-07-02_10-00-00.md");
        for path in [&words, &picture, &next] {
            fresh(path);
        }
        fresh(&notes.join("note-2026-10-30_09-00-00.md"));
        let now = local("2026-11-03_12-00-00");
        let stubborn = NoPictures(TestBin(dir.join("bin")));

        // The picture goes first; it won't, so its words stay with it, and the sweep stops there.
        let done = sweep(&places, now, &stubborn);
        assert_eq!((done.to_bin, done.failed), (0, 0));
        assert!(done.notes_stay.is_some_and(|why| why.contains("note-2026-07-01_10-00-00.png")));
        assert!(words.exists() && picture.exists() && next.exists());
        // At the next start the note after it goes; that one isn't tried again.
        let done = sweep(&places, now, &stubborn);
        assert_eq!(done, Tidied { to_bin: 1, to_bin_bytes: 1, ..Tidied::default() });
        assert!(words.exists() && picture.exists() && !next.exists());
        assert_eq!(sweep(&places, now, &TestBin(dir.join("bin"))), Tidied::default());
        assert!(words.exists() && picture.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn notes_stay_while_what_says_how_far_the_sweep_went_cant_be_read() {
        let dir = test_folder("done-file");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let notes = places.data.join("notes");
        let note = notes.join("note-2026-07-01_10-00-00.md");
        fresh(&note);
        fresh(&notes.join("note-2026-10-30_09-00-00.md"));
        let now = local("2026-11-03_12-00-00");
        let done_file = places.data.join(NOTES_DONE_FILE);

        // A folder of that name isn't the app's: it's left as it is, and so are the notes.
        fs::create_dir_all(&done_file).unwrap();
        let done = sweep(&places, now, &bin);
        assert_eq!(done.notes_stay, Some(format!("{NOTES_DONE_FILE} in the data folder couldn't be read")));
        assert!(done.to_bin == 0 && note.exists() && done_file.is_dir());
        fs::remove_dir(&done_file).unwrap();

        // Cut short by a crash: how far earlier sweeps went isn't known, so the notes old now stay (one may have been
        // put back), and it's written anew.
        fs::write(&done_file, "2026-0").unwrap();
        let done = sweep(&places, now, &bin);
        assert!(done.to_bin == 0 && done.notes_stay.is_some() && note.exists());
        assert_eq!(fs::read_to_string(&done_file).unwrap().lines().next(), Some("2026-08-01_09-00-00"));
        // Bytes that aren't text at all: the same.
        fs::write(&done_file, [0xFF, 0xFE, 0x00]).unwrap();
        let done = sweep(&places, now, &bin);
        assert!(done.to_bin == 0 && done.notes_stay.is_some_and(|why| why.contains("held something else")));
        assert_eq!(fs::read_to_string(&done_file).unwrap().lines().next(), Some("2026-08-01_09-00-00"));
        // Notes that grow old from then on go as usual.
        fresh(&notes.join("note-2026-09-01_10-00-00.md"));
        fresh(&notes.join("note-2026-12-20_10-00-00.md"));
        assert_eq!(sweep(&places, local("2027-01-01_12-00-00"), &bin).to_bin, 1);
        assert!(note.exists() && !notes.join("note-2026-09-01_10-00-00.md").exists());
        // Changed in an editor that puts a byte-order mark first: still read.
        fs::write(&done_file, "\u{feff}2026-07-01_10-00-00\r\n").unwrap();
        fresh(&notes.join("note-2026-08-20_10-00-00.md"));
        assert_eq!(sweep(&places, local("2027-01-01_12-00-00"), &bin).to_bin, 1);
        assert!(note.exists() && !notes.join("note-2026-08-20_10-00-00.md").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_newest_note_a_few_hours_ahead_still_counts_but_old_runs_from_now() {
        let dir = test_folder("hours-ahead");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let notes = places.data.join("notes");
        // Now is the 3rd of November at 12:00; the newest note was saved at 18:00 that day, on a clock 6 hours fast.
        // So the notes saved by the 5th of August at 12:00 are 3 months old, not those by 18:00.
        let goes = notes.join("note-2026-08-05_11-00-00.md");
        let stays = notes.join("note-2026-08-05_15-00-00.md");
        for path in [&goes, &stays, &notes.join("note-2026-11-03_18-00-00.md")] {
            fresh(path);
        }
        let done = sweep(&places, local("2026-11-03_12-00-00"), &bin);
        assert_eq!(done, Tidied { to_bin: 1, to_bin_bytes: 1, ..Tidied::default() });
        assert!(!goes.exists() && stays.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// The first three notes saved in July and the newest in October: the ones the tests below send.
    fn three_old_notes(notes: &Path) -> [PathBuf; 3] {
        let old = ["01", "02", "03"].map(|day| notes.join(format!("note-2026-07-{day}_10-00-00.md")));
        for path in &old {
            fresh(path);
        }
        fresh(&notes.join("note-2026-10-30_09-00-00.md"));
        old
    }

    /// The first line of what says how far the sweep went, and whether it says no more go by themselves.
    fn how_far(done_file: &Path) -> (String, bool) {
        let text = fs::read_to_string(done_file).unwrap();
        (text.lines().next().unwrap_or_default().to_owned(), text.lines().any(|l| l.starts_with(STOPPED)))
    }

    #[test]
    fn no_more_notes_go_by_themselves_once_windows_has_asked() {
        let dir = test_folder("asks");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let now = local("2026-11-03_12-00-00");
        let done_file = places.data.join(NOTES_DONE_FILE);

        // Windows asks "permanently delete?" about the first, and is told no: it stays, and so do the rest.
        let asks = OddBin { bin: TestBin(dir.join("bin")), odd: Odd::Asks };
        let done = sweep(&places, now, &asks);
        assert_eq!(done.to_bin, 0);
        assert!(done.notes_stopped);
        assert!(done.notes_stay.is_some_and(|why| why.contains("Windows didn't put note-2026-07-01_10-00-00.md")));
        assert!(first.exists() && second.exists() && third.exists());
        assert_eq!(how_far(&done_file), ("2026-07-01_10-00-00".to_owned(), true));

        // From then on, none go by themselves, even to a Recycle Bin that never asks.
        let bin = TestBin(dir.join("bin"));
        let done = sweep(&places, now, &bin);
        assert_eq!(done.to_bin, 0);
        assert!(done.notes_stay.is_some_and(|why| why.contains("none go by themselves any more")));
        assert!(second.exists() && third.exists() && !bin.0.exists());

        // With that line taken out, the rest go; the one you chose to keep stays.
        let text = fs::read_to_string(&done_file).unwrap();
        let kept: Vec<&str> = text.lines().filter(|l| !l.starts_with(STOPPED)).collect();
        fs::write(&done_file, kept.join("\n")).unwrap();
        assert_eq!(sweep(&places, now, &bin), Tidied { to_bin: 2, to_bin_bytes: 2, ..Tidied::default() });
        assert!(first.exists() && !second.exists() && !third.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_more_notes_go_by_themselves_once_one_isnt_in_the_bin_after() {
        let dir = test_folder("loses");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let picture = first.with_extension("png");
        fresh(&picture);
        let now = local("2026-11-03_12-00-00");
        let done_file = places.data.join(NOTES_DONE_FILE);

        // The first note's picture goes, but the Recycle Bin hasn't got it after (Windows asked, and was told yes):
        // nothing more goes, not even that note's words.
        let loses = OddBin { bin: TestBin(dir.join("bin")), odd: Odd::Loses };
        let done = sweep(&places, now, &loses);
        assert_eq!((done.to_bin, done.to_bin_bytes), (0, 0), "not counted as in the bin");
        assert!(done.notes_stay.is_some_and(|why| why.contains("note-2026-07-01_10-00-00.png couldn't be found")));
        assert!(!picture.exists() && first.exists() && second.exists() && third.exists());
        assert_eq!(how_far(&done_file), ("2026-07-01_10-00-00".to_owned(), true));
        assert_eq!(sweep(&places, now, &TestBin(dir.join("bin"))).to_bin, 0);
        assert!(second.exists() && third.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_gone_though_windows_didnt_move_it_stops_them_all() {
        let dir = test_folder("gone-but-kept");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let done_file = places.data.join(NOTES_DONE_FILE);
        // Not "another copy of the app sent it first": it may have been removed for good.
        let odd = OddBin { bin: TestBin(dir.join("bin")), odd: Odd::GoneButKept };
        let done = sweep(&places, local("2026-11-03_12-00-00"), &odd);
        assert!(done.notes_stopped && done.to_bin == 0);
        assert!(done.notes_stay.is_some_and(|why| why.contains("is gone from the notes folder")));
        assert!(!first.exists() && second.exists() && third.exists());
        assert_eq!(how_far(&done_file), ("2026-07-01_10-00-00".to_owned(), true));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_bin_is_looked_in_once_a_start_and_a_picture_goes_before_its_words() {
        let dir = test_folder("watched");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, ..] = three_old_notes(&notes);
        fresh(&first.with_extension("png"));
        let watched = WatchedBin {
            bin: TestBin(dir.join("bin")),
            sent: std::cell::RefCell::new(Vec::new()),
            looked: std::cell::Cell::new(0),
        };
        assert_eq!(sweep(&places, local("2026-11-03_12-00-00"), &watched).to_bin, 4);
        assert_eq!(watched.looked.get(), 1, "only the first file sent is looked for");
        let sent = watched.sent.borrow();
        assert_eq!(&sent[..2], ["note-2026-07-01_10-00-00.png", "note-2026-07-01_10-00-00.md"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stop_another_copy_of_the_app_writes_holds() {
        let dir = test_folder("other-copy");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let done_file = places.data.join(NOTES_DONE_FILE);
        let other = OtherCopyStops { bin: TestBin(dir.join("bin")), done_file: done_file.clone() };
        // This copy is sending its first note when the other stops them: it sends no more, and the line stays.
        let done = sweep(&places, local("2026-11-03_12-00-00"), &other);
        assert_eq!(done.to_bin, 1);
        assert!(done.notes_stay.is_some_and(|why| why.contains("the other copy's reason")));
        assert!(!first.exists() && second.exists() && third.exists());
        assert!(how_far(&done_file).1, "the other copy's line is still there");
        let _ = fs::remove_dir_all(&dir);

        // The same while it sends the last note it has: what it writes once it's done leaves the line too.
        let dir = test_folder("other-copy-last");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        fresh(&notes.join("note-2026-07-01_10-00-00.md"));
        fresh(&notes.join("note-2026-10-30_09-00-00.md"));
        let done_file = places.data.join(NOTES_DONE_FILE);
        let other = OtherCopyStops { bin: TestBin(dir.join("bin")), done_file: done_file.clone() };
        let done = sweep(&places, local("2026-11-03_12-00-00"), &other);
        assert_eq!(done.to_bin, 1);
        assert!(done.notes_stay.is_some_and(|why| why.contains("the other copy's reason")));
        assert!(how_far(&done_file).1, "the other copy's line is still there");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_another_copy_of_the_app_has_looked_at_meanwhile_stays() {
        let dir = test_folder("copy-ahead");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let done_file = places.data.join(NOTES_DONE_FILE);
        // While this copy sends the first note, the other gets past the second (which you put back from the bin, say).
        let other = OtherCopyAhead {
            bin: TestBin(dir.join("bin")),
            done_file: done_file.clone(),
            ahead_to: "2026-07-02_12-00-00",
            said: std::cell::Cell::new(false),
        };
        let done = sweep(&places, local("2026-11-03_12-00-00"), &other);
        assert_eq!(done.to_bin, 2);
        assert!(!first.exists() && second.exists() && !third.exists(), "the one the other copy looked at stays");
        assert_eq!(how_far(&done_file).0, "2026-08-01_09-00-00");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn how_far_the_sweep_went_never_goes_back() {
        let dir = test_folder("never-back");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let done_file = places.data.join(NOTES_DONE_FILE);
        // While this copy sends its first note, the other gets past every note this one would send, and further: it
        // saw a newer note (one saved since, say). That later time stays, so a note you put back meanwhile stays too.
        let other = OtherCopyAhead {
            bin: TestBin(dir.join("bin")),
            done_file: done_file.clone(),
            ahead_to: "2026-08-05_09-00-00",
            said: std::cell::Cell::new(false),
        };
        let done = sweep(&places, local("2026-11-03_12-00-00"), &other);
        assert_eq!(done, Tidied { to_bin: 1, to_bin_bytes: 1, ..Tidied::default() });
        assert!(!first.exists() && second.exists() && third.exists());
        assert_eq!(how_far(&done_file).0, "2026-08-05_09-00-00");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stop_written_while_a_note_is_looked_at_holds() {
        let dir = test_folder("stop-while-looking");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let done_file = places.data.join(NOTES_DONE_FILE);
        // The "Stopped" line goes in while this copy asks whether the Recycle Bin can take its first note quietly
        // (on Windows that reads the registry, the drive and the bin, so it takes a moment): nothing goes, and the
        // line stays.
        let bin = StopsWhileLooking { bin: TestBin(dir.join("bin")), done_file: done_file.clone(), folder: notes };
        let done = sweep(&places, local("2026-11-03_12-00-00"), &bin);
        assert_eq!(done.to_bin, 0);
        assert!(done.notes_stay.is_some_and(|why| why.contains("your reason")));
        assert!(first.exists() && second.exists() && third.exists());
        assert!(how_far(&done_file).1, "the line is still there");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn notes_wait_while_another_copy_of_the_app_goes_through_them() {
        let dir = test_folder("lock");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let now = local("2026-11-03_12-00-00");
        let done_file = places.data.join(NOTES_DONE_FILE);
        let lock = places.data.join(NOTES_LOCK_FILE);

        // Another copy holds the lock: this one leaves the notes to it, and writes nothing down.
        let other = File::create(&lock).unwrap();
        other.lock().unwrap();
        let done = sweep(&places, now, &bin);
        let busy = Some("another copy of the app is going through them now".to_owned());
        assert_eq!(done, Tidied { notes_stay: busy.clone(), ..Tidied::default() });
        assert!(first.exists() && second.exists() && third.exists() && !done_file.exists());
        // Not even what says how far the sweep went, when it can't be read (it would be written anew otherwise).
        fs::write(&done_file, "2026-0").unwrap();
        assert_eq!(sweep(&places, now, &bin), Tidied { notes_stay: busy, ..Tidied::default() });
        assert_eq!(fs::read_to_string(&done_file).unwrap(), "2026-0");
        fs::remove_file(&done_file).unwrap();
        // Once it's done, they go, holding the lock while each goes; and the lock stays as it was, empty.
        drop(other);
        let checks = ChecksTheLock { bin: TestBin(dir.join("bin")), lock: lock.clone(), held: std::cell::Cell::new(0) };
        assert_eq!(sweep(&places, now, &checks), Tidied { to_bin: 3, to_bin_bytes: 3, ..Tidied::default() });
        assert_eq!(checks.held.get(), 3, "the lock is held while each note goes");
        assert_eq!(fs::metadata(&lock).unwrap().len(), 0);
        let _ = fs::remove_dir_all(&dir);

        // A folder of that name isn't the app's: it's left as it is, and so are the notes.
        let dir = test_folder("lock-folder");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let [first, ..] = three_old_notes(&places.data.join("notes"));
        fs::create_dir_all(places.data.join(NOTES_LOCK_FILE)).unwrap();
        let done = sweep(&places, now, &TestBin(dir.join("bin")));
        assert!(done.to_bin == 0 && first.exists() && places.data.join(NOTES_LOCK_FILE).is_dir());
        assert!(done.notes_stay.is_some_and(|why| why.contains(NOTES_LOCK_FILE)));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Mac, Linux: a link of the lock's name is never opened through.
    #[cfg(unix)]
    #[test]
    fn the_lock_is_never_taken_through_a_link() {
        use std::os::unix::fs::symlink;
        let dir = test_folder("lock-link");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let [first, ..] = three_old_notes(&places.data.join("notes"));
        let lock = places.data.join(NOTES_LOCK_FILE);
        let now = local("2026-11-03_12-00-00");
        // A link to a file of yours: the notes stay, and your file is never opened.
        let yours = dir.join("yours.txt");
        fs::write(&yours, "yours").unwrap();
        symlink(&yours, &lock).unwrap();
        let done = sweep(&places, now, &TestBin(dir.join("bin")));
        assert!(done.to_bin == 0 && first.exists());
        assert!(done.notes_stay.is_some_and(|why| why.contains(NOTES_LOCK_FILE)));
        assert_eq!(fs::read_to_string(&yours).unwrap(), "yours");
        // A link to nothing: the same, and nothing is made where it leads.
        let nowhere = dir.join("nowhere.txt");
        fs::remove_file(&lock).unwrap();
        symlink(&nowhere, &lock).unwrap();
        let done = sweep(&places, now, &TestBin(dir.join("bin")));
        assert!(done.to_bin == 0 && first.exists());
        assert!(done.notes_stay.is_some_and(|why| why.contains(NOTES_LOCK_FILE)));
        assert!(!nowhere.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_that_cant_be_looked_at_stays_and_the_rest_go() {
        let dir = test_folder("cant-look");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let now = local("2026-11-03_12-00-00");
        // Whether the first is all on this computer couldn't be read: its own reason. It stays, looked at, and the
        // others go.
        let picky = PickyBin {
            bin: TestBin(dir.join("bin")),
            refuses: |path, _| path.file_name()?.to_str()?.starts_with("note-2026-07-01").then(|| CANT_LOOK.into()),
        };
        let done = sweep(&places, now, &picky);
        assert_eq!(
            done,
            Tidied { to_bin: 2, to_bin_bytes: 2, notes_stay: Some(CANT_LOOK.to_owned()), ..Tidied::default() }
        );
        assert!(first.exists() && !second.exists() && !third.exists());
        // It was looked at once, so it isn't tried again.
        assert_eq!(sweep(&places, now, &TestBin(dir.join("bin"))), Tidied::default());
        assert!(first.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_more_notes_go_by_themselves_once_one_is_still_there_after_windows_moved_it() {
        let now = local("2026-11-03_12-00-00");
        for odd in [Odd::StillThere, Odd::Lingers] {
            let dir = test_folder("still-there");
            let places = Places { data: dir.join("data"), temp: dir.join("temp") };
            let notes = places.data.join("notes");
            let [first, second, third] = three_old_notes(&notes);
            let done_file = places.data.join(NOTES_DONE_FILE);
            // Windows said it moved the first, but it was still there (it may have asked, and been told yes, while a
            // program had it open): not counted as in the bin, and none go by themselves from then on, even when it's
            // gone a moment later.
            let done = sweep(&places, now, &OddBin { bin: TestBin(dir.join("bin")), odd });
            assert!(done.notes_stopped && done.to_bin == 0);
            let said = if odd == Odd::StillThere { "but it's still in the notes folder" } else { "and is gone now" };
            assert!(done.notes_stay.is_some_and(|why| why.contains(said)), "{odd:?}");
            assert_eq!(first.exists(), odd == Odd::StillThere);
            assert!(second.exists() && third.exists());
            assert_eq!(how_far(&done_file), ("2026-07-01_10-00-00".to_owned(), true));
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn writing_down_a_stop_is_tried_again_a_moment_later() {
        let tries = std::cell::Cell::new(0);
        let fails_twice = || {
            tries.set(tries.get() + 1);
            if tries.get() < 3 { Err(std::io::Error::other("a virus scanner has it")) } else { Ok(()) }
        };
        assert!(tried(fails_twice).is_ok());
        assert_eq!(tries.get(), 3);
        // Once it works, it isn't written again; and it gives up after the third try.
        tries.set(0);
        let works = || {
            tries.set(tries.get() + 1);
            Ok(())
        };
        assert!(tried(works).is_ok());
        assert_eq!(tries.get(), 1);
        tries.set(0);
        assert!(tried(|| Err(std::io::Error::other(format!("try {}", tries.replace(tries.get() + 1))))).is_err());
        assert_eq!(tries.get(), STOP_TRIES);
    }

    #[test]
    fn a_stop_that_cant_be_written_down_says_notes_may_go_again() {
        let dir = test_folder("stop-unwritten");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let done_file = places.data.join(NOTES_DONE_FILE);
        let part = places.data.join(format!("{NOTES_DONE_FILE}{PART_ENDING}"));
        // Windows asks about the first, and from then on the stop can't be written down, however often it's tried.
        let done = sweep(&places, local("2026-11-03_12-00-00"), &AsksAndCantBeWritten { part: part.clone() });
        assert!(done.notes_stopped && done.to_bin == 0);
        assert!(done.notes_stay.is_some_and(|why| why.contains("so they may go again at the next start")));
        assert!(first.exists() && second.exists() && third.exists());
        assert_eq!(how_far(&done_file), ("2026-07-01_10-00-00".to_owned(), false));
        assert!(part.is_dir(), "what was in the way is left as it is");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn notes_wait_for_the_next_start_while_the_bin_cant_be_read() {
        let dir = test_folder("unread-bin");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let picture = first.with_extension("png");
        fresh(&picture);
        let now = local("2026-11-03_12-00-00");
        let done_file = places.data.join(NOTES_DONE_FILE);

        // Whether the first note's picture is in the bin can't be seen: its words go with it, and the rest wait.
        let unread = OddBin { bin: TestBin(dir.join("bin")), odd: Odd::CantBeRead };
        let done = sweep(&places, now, &unread);
        assert_eq!((done.to_bin, done.to_bin_bytes), (2, 2));
        assert!(done.notes_stay.is_some_and(|why| why.contains("couldn't be read")));
        assert!(!picture.exists() && !first.exists() && second.exists() && third.exists());
        assert_eq!(how_far(&done_file), ("2026-07-01_10-00-00".to_owned(), false));
        // At the next start, with the bin read, they go.
        assert_eq!(sweep(&places, now, &TestBin(dir.join("bin"))).to_bin, 2);
        assert!(!second.exists() && !third.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn notes_the_sweep_hadnt_reached_when_the_app_closed_go_at_the_next_start() {
        let dir = test_folder("closed");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let now = local("2026-11-03_12-00-00");
        let done_file = places.data.join(NOTES_DONE_FILE);

        // The app closes as the second note is being put in the bin.
        let closing = ClosingBin { bin: TestBin(dir.join("bin")), sent: std::cell::Cell::new(0) };
        let cut_off = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sweep(&places, now, &closing)));
        assert!(cut_off.is_err());
        assert!(!first.exists() && second.exists() && third.exists());
        // The second may have gone (and been put back since), so it counts as looked at; the third doesn't.
        assert_eq!(how_far(&done_file), ("2026-07-02_10-00-00".to_owned(), false));
        assert_eq!(
            sweep(&places, now, &TestBin(dir.join("bin"))),
            Tidied { to_bin: 1, to_bin_bytes: 1, ..Tidied::default() }
        );
        assert!(second.exists() && !third.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn notes_wait_while_what_says_how_far_the_sweep_went_is_after_now() {
        let dir = test_folder("done-ahead");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let notes = places.data.join("notes");
        let note = notes.join("note-2026-07-01_10-00-00.md");
        fresh(&note);
        fresh(&notes.join("note-2026-10-30_09-00-00.md"));
        let done_file = places.data.join(NOTES_DONE_FILE);
        // Written in March next year: the clock is wrong now (put back), or was then.
        fs::write(&done_file, "2027-03-01_08-00-00\n").unwrap();
        // While no note is old enough, there's nothing to say.
        assert_eq!(sweep(&places, local("2026-09-15_12-00-00"), &bin), Tidied::default());
        let done = sweep(&places, local("2026-11-03_12-00-00"), &bin);
        assert_eq!(done.to_bin, 0);
        assert!(done.notes_stay.is_some_and(|why| why.contains("clock may be wrong")));
        assert!(note.exists());
        assert_eq!(fs::read_to_string(&done_file).unwrap(), "2027-03-01_08-00-00\n", "left as it is");
        // Once the clock is past it, notes saved after it go as usual; those before it count as looked at.
        let march = notes.join("note-2027-03-10_10-00-00.md");
        fresh(&march);
        fresh(&notes.join("note-2027-06-20_10-00-00.md"));
        assert_eq!(sweep(&places, local("2027-07-01_12-00-00"), &bin).to_bin, 1);
        assert!(note.exists() && !march.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_gone_before_its_turn_is_neither_counted_nor_a_reason_to_stop() {
        let dir = test_folder("gone-before");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let done_file = places.data.join(NOTES_DONE_FILE);
        // You remove the second while the first goes: it's passed over, and the third goes.
        let bin = YouRemoveTheNext { bin: TestBin(dir.join("bin")), next: second.clone() };
        let done = sweep(&places, local("2026-11-03_12-00-00"), &bin);
        assert_eq!(done, Tidied { to_bin: 2, to_bin_bytes: 2, ..Tidied::default() });
        assert!(!first.exists() && !second.exists() && !third.exists());
        assert_eq!(how_far(&done_file), ("2026-08-01_09-00-00".to_owned(), false));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_gone_after_windows_said_it_couldnt_go_stops_them_all() {
        let dir = test_folder("gone-after");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let notes = places.data.join("notes");
        let [first, second, third] = three_old_notes(&notes);
        let done_file = places.data.join(NOTES_DONE_FILE);
        // Only one copy of the app goes through the notes at a time, so it wasn't another copy that sent it: Windows
        // may have removed it for good.
        let done = sweep(&places, local("2026-11-03_12-00-00"), &GoneAfterAnError);
        assert!(done.notes_stopped && done.to_bin == 0);
        assert!(done.notes_stay.is_some_and(|why| why.contains("is gone from the notes folder")));
        assert!(!first.exists() && second.exists() && third.exists());
        assert_eq!(how_far(&done_file), ("2026-07-01_10-00-00".to_owned(), true));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A link of that name is never read or written through.
    #[cfg(unix)]
    #[test]
    fn what_says_how_far_the_sweep_went_is_never_written_through_a_link() {
        use std::os::unix::fs::symlink;
        let dir = test_folder("done-link");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let notes = places.data.join("notes");
        fresh(&notes.join("note-2026-07-01_10-00-00.md"));
        fresh(&notes.join("note-2026-10-30_09-00-00.md"));
        let yours = dir.join("yours.txt");
        fs::write(&yours, "yours").unwrap();
        symlink(&yours, places.data.join(NOTES_DONE_FILE)).unwrap();
        let done = sweep(&places, local("2026-11-03_12-00-00"), &bin);
        assert!(done.to_bin == 0 && done.notes_stay.is_some());
        assert_eq!(fs::read_to_string(&yours).unwrap(), "yours");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Mac, Linux: what says how far the sweep went is put in its place whole, never written into, so a crash part
    /// way leaves the old one as it was (with its "Stopped" line, if it has one).
    #[cfg(unix)]
    #[test]
    fn what_says_how_far_the_sweep_went_is_put_in_place_whole() {
        use std::io::Read as _;
        let dir = test_folder("done-whole");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let [first, ..] = three_old_notes(&places.data.join("notes"));
        let done_file = places.data.join(NOTES_DONE_FILE);
        fs::write(&done_file, "2026-06-01_00-00-00\n").unwrap();
        // What a crash part way through an earlier write left beside it.
        let part = places.data.join(format!("{NOTES_DONE_FILE}{PART_ENDING}"));
        fs::write(&part, "2026-0").unwrap();
        // The old one, held open: if it were written into, this would read the new words.
        let mut old = File::open(&done_file).unwrap();
        let done = sweep(&places, local("2026-11-03_12-00-00"), &bin);
        assert_eq!(done.to_bin, 3);
        assert!(!first.exists());
        let mut held = String::new();
        old.read_to_string(&mut held).unwrap();
        assert_eq!(held, "2026-06-01_00-00-00\n", "the old one was never written into");
        assert_eq!(how_far(&done_file).0, "2026-08-01_09-00-00");
        assert!(!part.exists(), "nothing is left beside it");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Mac, Linux: writing what says how far the sweep went never writes through a link of its name, or of the name
    /// it's first written to: the link is replaced, and what it leads to stays as it was.
    #[cfg(unix)]
    #[test]
    fn what_says_how_far_the_sweep_went_is_written_beside_and_never_through_a_link() {
        use std::os::unix::fs::symlink;
        let dir = test_folder("write-links");
        fs::create_dir_all(&dir).unwrap();
        let yours = dir.join("yours.txt");
        let also_yours = dir.join("also yours.txt");
        fs::write(&yours, "yours").unwrap();
        fs::write(&also_yours, "also yours").unwrap();
        let done_file = dir.join(NOTES_DONE_FILE);
        symlink(&yours, &done_file).unwrap();
        symlink(&also_yours, dir.join(format!("{NOTES_DONE_FILE}{PART_ENDING}"))).unwrap();
        let up_to = NaiveDateTime::parse_from_str("2026-08-01_09-00-00", NAME_TIME).unwrap();
        write_notes_done(&done_file, up_to, Some("a reason")).unwrap();
        assert_eq!(fs::read_to_string(&yours).unwrap(), "yours");
        assert_eq!(fs::read_to_string(&also_yours).unwrap(), "also yours");
        assert!(fs::symlink_metadata(&done_file).unwrap().is_file());
        assert_eq!(how_far(&done_file), ("2026-08-01_09-00-00".to_owned(), true));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_copy_keeps_its_old_changed_time_but_is_as_new_as_when_it_was_made() {
        let dir = test_folder("copied");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let now = SystemTime::now();
        let list = places.temp.join("mo-admin-1-2-3.txt");
        // As a copy is: made now, last changed long ago.
        changed_at(&list, now - DAY * 2500);
        // A Mac can set when it was made back too, as a copy that keeps its times may. Then only when its entry last
        // changed (setting a time is a change) keeps it new, as it keeps new a file moved in with its old made time.
        #[cfg(target_os = "macos")]
        {
            use std::os::macos::fs::FileTimesExt as _;
            let made_long_ago = fs::FileTimes::new().set_created(now - DAY * 2500);
            File::options().write(true).open(&list).unwrap().set_times(made_long_ago).unwrap();
        }
        // Windows keeps when a file was made; a Mac and Linux when its entry last changed (setting a time is a change).
        let made_is_known = cfg!(unix) || fs::metadata(&list).unwrap().created().is_ok();
        let done = sweep(&places, now + MINUTE, &bin);
        if made_is_known {
            assert_eq!(done, Tidied::default());
            assert!(list.exists(), "made a minute ago, so not a day old");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_launchers_folders_and_folders_inside_are_never_gone_into() {
        let dir = test_folder("launcher");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let logs = places.data.join("logs");
        // Files named as the sweep clears them, but inside folders: the launcher's, beside the code, and wherever
        // -BuildDir put a build folder (the data folder, its logs, the Temp folder), and a folder the app might once
        // have used in Temp. In the folders inside logs, more than the limits keep.
        let mut inside = vec![
            places.data.join("run-latest-rust").join("run").join("20261101-120000").join("music-organiser.exe"),
            places.data.join("run-latest-rust").join("app").join("target").join("debug").join("crash-1.log"),
            places.data.join("run-latest-rust").join("backup.txt.mo-part"),
            places.data.join("run-latest-rust").join("notes").join("note-2020-01-01_00-00-00.md"),
            places.data.join("run-latest-rust").join("notes").join("note-2020-06-01_00-00-00.md"),
            places.data.join("MusicOrganiser-build").join("debug").join("presets.db.new-1"),
            places.data.join("notes").join("old").join("note-2020-01-01_00-00-00.md"),
            places.data.join("notes").join("old").join("note-2020-06-01_00-00-00.md"),
            places.data.join("twin").join("backup.txt.mo-part"),
            places.temp.join("MusicOrganiser-build").join("mo-admin-1-2-3.txt"),
            places.temp.join("MusicOrganiser-Rust").join("mo-admin-1-2-3.txt"),
        ];
        for i in 0..40u32 {
            inside.push(logs.join("MusicOrganiser-build").join(format!("crash-{}.log", 1_700_000_000 + i)));
            inside.push(logs.join("old").join(format!("rust-session-2020-01-01_00-00-{i:02}.log")));
            inside.push(logs.join("old").join(format!("twin-report-2020-01-01-{i:04}.txt")));
        }
        for path in &inside {
            fresh(path);
        }
        // Something of the sweep's own beside them, so it's seen to run.
        fresh(&places.temp.join("mo-admin-9-9-9.txt"));

        let far = SystemTime::now() + DAY * 4000;
        assert_eq!(sweep(&places, far, &bin), Tidied { removed: 1, bytes: 1, ..Tidied::default() });
        assert_eq!(prune_logs(&logs, None, far), Tidied::default());
        for path in &inside {
            assert!(path.exists(), "{} stays", path.display());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// Links named as the app names its files, and folders that are links: nothing goes, here or where they lead.
    #[cfg(unix)]
    #[test]
    fn nothing_is_removed_through_a_link() {
        use std::os::unix::fs::symlink;
        let dir = test_folder("links");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let yours = dir.join("yours");
        let song = yours.join("Song.flp");
        fresh(&song);
        // With a newer note beside it, the old note would go if the notes link were followed.
        let named = [
            "library-copy.db",
            "x.mo-part-1-2",
            "backup.txt.mo-part",
            "note-2000-01-01_00-00-00.md",
            "note-2000-06-01_00-00-00.md",
        ];
        for name in named {
            fresh(&yours.join("cs").join(name));
            fresh(&yours.join(name));
        }
        fs::create_dir_all(places.data.join("logs")).unwrap();
        fs::create_dir_all(&places.temp).unwrap();
        symlink(&song, places.data.join("logs").join("crash-1700000000.log")).unwrap();
        for i in 0..40 {
            symlink(&song, places.data.join("logs").join(format!("twin-test-2026-10-01-{i:04}.txt"))).unwrap();
            symlink(&song, places.data.join("logs").join(format!("rust-session-2000-01-01_00-00-{i:02}.log"))).unwrap();
        }
        symlink(&song, places.data.join("backup.txt.mo-part")).unwrap();
        symlink(&song, places.temp.join("mo-admin-1-2-3.txt")).unwrap();
        symlink(&yours, places.data.join("twin")).unwrap();
        symlink(&yours, places.data.join(CURVES)).unwrap();
        symlink(&yours, places.data.join("notes")).unwrap();

        let far = SystemTime::now() + DAY * 4000;
        assert_eq!(sweep(&places, far, &bin), Tidied::default());
        assert_eq!(prune_logs(&places.data.join("logs"), None, far), Tidied::default());
        assert!(song.exists());
        for name in named {
            assert!(yours.join("cs").join(name).exists() && yours.join(name).exists(), "{name}");
        }
        assert!(fs::symlink_metadata(places.temp.join("mo-admin-1-2-3.txt")).is_ok(), "the link itself stays too");
        assert_eq!(names(&places.data.join("logs")).len(), 81, "the links in logs stay");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The app writes its logs through a logs folder that is a link (moved to another drive), so they're kept in
    /// bounds there; a note that is a link, in a notes folder that isn't, never goes.
    #[cfg(unix)]
    #[test]
    fn logs_are_kept_in_bounds_through_a_linked_folder() {
        use std::os::unix::fs::symlink;
        let dir = test_folder("linked-logs");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let elsewhere = dir.join("D").join("logs");
        for i in 0..40 {
            fresh(&elsewhere.join(format!("rust-session-2000-01-01_00-00-{i:02}.log")));
        }
        fs::create_dir_all(&places.data).unwrap();
        symlink(&elsewhere, places.data.join("logs")).unwrap();
        let song = dir.join("yours").join("Song.flp");
        fresh(&song);
        // A newer note of its own beside it, so the link would be old enough to go if it counted as a note.
        fresh(&places.data.join("notes").join("note-2000-06-01_00-00-00.md"));
        symlink(&song, places.data.join("notes").join("note-2000-01-01_00-00-00.md")).unwrap();

        let far = SystemTime::now() + DAY * 4000;
        let done = prune_logs(&places.data.join("logs"), None, far);
        assert_eq!(done.removed, 10);
        assert_eq!(names(&elsewhere).len(), crate::log::KEPT);
        assert_eq!(sweep(&places, far, &bin), Tidied::default());
        assert!(
            song.exists()
                && fs::symlink_metadata(places.data.join("notes").join("note-2000-01-01_00-00-00.md")).is_ok()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The same with Windows junctions (`mklink /J`, which needs no administrator rights).
    #[cfg(windows)]
    #[test]
    fn nothing_is_removed_through_a_junction() {
        let dir = test_folder("junctions");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let yours = dir.join("yours");
        let junction = |at: &Path, to: &Path| {
            let made = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(at).arg(to).output();
            assert!(made.is_ok_and(|o| o.status.success()), "mklink /J {}", at.display());
        };
        let named = ["library-copy.db", "x.mo-part-1-2", "note-2000-01-01_00-00-00.md", "note-2000-06-01_00-00-00.md"];
        for name in named {
            fresh(&yours.join("cs").join(name));
            fresh(&yours.join(name));
        }
        fs::create_dir_all(places.data.join("logs")).unwrap();
        fs::create_dir_all(&places.temp).unwrap();
        junction(&places.data.join("twin"), &yours);
        junction(&places.data.join(CURVES), &yours);
        junction(&places.data.join("notes"), &yours);
        junction(&places.data.join("logs").join("crash-1700000000.log"), &yours);
        junction(&places.temp.join("mo-admin-1-2-3.txt"), &yours);

        let far = SystemTime::now() + DAY * 4000;
        assert_eq!(sweep(&places, far, &bin), Tidied::default());
        assert_eq!(prune_logs(&places.data.join("logs"), None, far), Tidied::default());
        for name in named {
            assert!(yours.join("cs").join(name).exists() && yours.join(name).exists(), "{name}");
        }
        assert!(fs::symlink_metadata(places.temp.join("mo-admin-1-2-3.txt")).is_ok(), "the junction itself stays too");
        // Junctions first, so removing the test folder never reaches through them.
        for at in [
            places.data.join("twin"),
            places.data.join(CURVES),
            places.data.join("notes"),
            places.data.join("logs").join("crash-1700000000.log"),
            places.temp.join("mo-admin-1-2-3.txt"),
        ] {
            let _ = fs::remove_dir(at);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// A logs folder that is a junction (moved to another drive) is where the app writes its logs: kept in bounds.
    #[cfg(windows)]
    #[test]
    fn logs_are_kept_in_bounds_through_a_junction() {
        let dir = test_folder("junction-logs");
        let elsewhere = dir.join("D").join("logs");
        for i in 0..40 {
            fresh(&elsewhere.join(format!("rust-session-2000-01-01_00-00-{i:02}.log")));
        }
        fs::create_dir_all(dir.join("data")).unwrap();
        let logs = dir.join("data").join("logs");
        let made =
            std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&logs).arg(&elsewhere).output();
        assert!(made.is_ok_and(|o| o.status.success()), "mklink /J {}", logs.display());
        let done = prune_logs(&logs, None, SystemTime::now() + DAY * 4000);
        assert_eq!(done.removed, 10);
        assert_eq!(names(&elsewhere).len(), crate::log::KEPT);
        let _ = fs::remove_dir(&logs);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_there_is_nothing_to_do() {
        let dir = test_folder("empty");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        assert_eq!(sweep(&places, SystemTime::now(), &bin), Tidied::default());
        assert_eq!(prune_logs(&places.data.join("logs"), None, SystemTime::now()), Tidied::default());
        assert!(!dir.exists(), "the sweep makes no folders");
    }

    #[test]
    fn names_are_read_whole() {
        assert!(is_report("galaxy-check-2026-11-02_14-30-05.txt", "galaxy-check-"));
        assert!(is_report("twin-test-2026-11-02-1430.txt", "twin-test-"));
        assert!(!is_report("midi-twin-report-2026-11-02-1430.txt", "midi-twin-"), "another kind");
        assert!(is_report("midi-twin-report-2026-11-02-1430.txt", "midi-twin-report-"));
        assert!(!is_report("twin-test-.txt", "twin-test-") && !is_report("twin-test-2026 copy.txt", "twin-test-"));
        assert!(is_cut_short("presets.db.new-12-wal") && is_cut_short("curves.db.new-3-shm"));
        assert!(is_cut_short("backup.txt.mo-part") && is_cut_short("backup-songs.txt.mo-part"));
        assert!(!is_cut_short("presets.db.new-12-old") && !is_cut_short("presets.db") && !is_cut_short("backup.txt"));
        assert!(!is_cut_short("library.db.new-12") && !is_cut_short("backup.txt.mo-part.2"));
        assert!(mo_presets::export::is_half_written("A.bwcurve.mo-part-1-0"));
        assert!(!mo_presets::export::is_half_written("A.bwcurve.mo-part-1-x"));
        assert!(!mo_presets::export::is_half_written("A.bwcurve.mo-part-1"));
        assert!(is_library_copy("library-copy.db-shm") && !is_library_copy("library-copy.db2"));
        assert_eq!(note_name("note-2026-11-02_14-30-05.md"), Some("note-2026-11-02_14-30-05"));
        assert_eq!(note_name("note-2026-11-02_14-30-05-2.png"), Some("note-2026-11-02_14-30-05-2"));
        let at = |s: &str| NaiveDateTime::parse_from_str(s, NAME_TIME).ok();
        assert_eq!(saved_at("note-2026-11-02_14-30-05"), at("2026-11-02_14-30-05"));
        assert_eq!(saved_at("note-2026-11-02_14-30-05-2"), at("2026-11-02_14-30-05"));
        assert_eq!(saved_at("note-2026-02-30_14-30-05"), None, "a day there isn't");
        assert_eq!(saved_at("note-2026-11-02_25-30-05"), None, "an hour there isn't");
        for name in [
            "note-2026-11-02_14-30-05.txt",
            "note-2026-11-02_14-30-05 - Copy.md",
            "note-2026-11-02_14-30-05-.md",
            "note-2026-11-02.md",
            "note-.md",
            "notes-2026-11-02_14-30-05.md",
            "note-2026-11-02_14-30-05.md.png",
            "draft.txt",
            "unsaved.txt",
            "writing.lock",
        ] {
            assert_eq!(note_name(name), None, "{name}");
        }
    }
}
