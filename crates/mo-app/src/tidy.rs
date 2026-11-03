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
//! copied in today is a day old tomorrow, not seven years old.
//!
//! **F12 notes** are the one thing here you made, so they go to the Recycle Bin rather than straight off the disk, and
//! only when it can take them without asking: Windows asks "permanently delete?" about a file it can't keep (on a
//! network drive or USB stick, or where the Recycle Bin is off or too small), and nobody is there to answer. A Mac
//! keeps them for now (its Trash is reached through Finder, which can ask too). The app never empties the Recycle Bin
//! itself (Solomon, 2026-11-03: "leave it to me"; the Space page says what's there). A note's age comes from the time
//! in its name, when it was saved, and runs to now or to the newest note, whichever is earlier, so a clock that jumps
//! months on (a flat battery, a date set by hand) makes no note old by itself. [`NOTES_DONE_FILE`] in the data folder
//! says how far the sweep has gone: it looks at each note once, so a note put back from the Recycle Bin stays, and so
//! does one the Recycle Bin wouldn't take (it's never tried again, and the notes after it wait for the next start, so
//! at most one is tried and fails at each start). Never the draft (`draft.txt`), the words of a save that failed
//! (`unsaved.txt`) or the lock (`writing.lock`) (rule 17).
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
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{NaiveDateTime, TimeDelta};
use mo_space::clean::Bin;

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
/// or the Recycle Bin wouldn't take it, so it stays.
pub const NOTES_DONE_FILE: &str = "tidy-notes.txt";

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
    /// The app's files that couldn't be removed (in use, or read-only): they're tried again next time.
    pub failed: usize,
    /// Why F12 notes old enough to go stayed, when some did.
    pub notes_stay: Option<String>,
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
    }

    /// Removes one file, counting it.
    fn remove(&mut self, path: &Path, bytes: u64) {
        match fs::remove_file(path) {
            Ok(()) => {
                self.removed += 1;
                self.bytes = self.bytes.saturating_add(bytes);
            }
            // Gone already (another copy of the app swept it first): nothing to do.
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            // Not this user's to remove (someone else's file in a shared temp folder): it isn't ours, so not a fault.
            Err(e) if e.kind() == ErrorKind::PermissionDenied && !cfg!(windows) => {}
            Err(_) => self.failed += 1,
        }
    }

    /// Puts one of your files in the bin, counting it; says why when it didn't go.
    fn send(&mut self, bin: &dyn Bin, path: &Path, bytes: u64) -> Result<(), String> {
        match bin.send(path) {
            Ok(()) => {
                self.to_bin += 1;
                self.to_bin_bytes = self.to_bin_bytes.saturating_add(bytes);
                Ok(())
            }
            // Gone already (another copy of the app sent it first): nothing to do.
            Err(_) if fs::symlink_metadata(path).is_err_and(|e| e.kind() == ErrorKind::NotFound) => Ok(()),
            Err(why) => Err(why),
        }
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
/// cleared. Never on the window's thread (rules 12 and 21).
pub fn start() {
    let started = crate::background::spawn("Tidy up", || {
        std::thread::sleep(WAIT);
        report(&sweep(&Places::this_pc(), SystemTime::now(), &mo_space::clean::SystemBin));
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
        crate::log::info("Tidy up", &format!("Some F12 notes over 3 months old stay: {why}"));
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
    done.add(remove_older(&places.data, is_cut_short, WRITING, now));
    if let Some(curves) = real_folder(&places.data, &[CURVES]) {
        done.add(remove_older(&curves, mo_presets::export::is_half_written, WRITING, now));
    }
    if let Some(twin) = real_folder(&places.data, &["twin", "cs"]) {
        done.add(remove_older(&twin, is_library_copy, DAY, now));
    }
    done.add(remove_older(&places.temp, mo_space::admin::is_temp_file, DAY, now));
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

/// Deletes all but the newest `keep` of `files`, newest by the latest of their times (see [`latest`]). A time up to
/// [`SKEW`] after `now` is taken as it is; one further on (a clock that was plainly wrong once) would hold its place
/// for ever, so it counts as the oldest. `spare`, the file just made, is never deleted and counts as one of the newest,
/// and neither is a file changed less than `live` before `now`.
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
    if files.len() <= keep {
        return done;
    }
    // Names alone put "…-05-2.log" before "…-05.log", so the time each was written decides.
    files.sort_by_cached_key(|(p, m)| (latest(m).filter(|t| *t <= now + SKEW), p.clone()));
    for (old, meta) in &files[..files.len() - keep] {
        let is_live = latest(meta).is_some_and(|t| t <= now + SKEW && now.duration_since(t).unwrap_or_default() < live);
        if !is_live {
            done.remove(old, meta.len());
        }
    }
    done
}

/// Deletes the plain files in `folder` whose names `is_kind` knows, once they're at least `age` old at `now` (see
/// [`age`]). One whose times can't be read stays, and so does one dated up to [`SKEW`] after `now`; one dated further
/// on (the clock was plainly wrong when it was written) is a leftover as old as can be.
fn remove_older(folder: &Path, is_kind: impl Fn(&str) -> bool, old_at: Duration, now: SystemTime) -> Tidied {
    let mut done = Tidied::default();
    for (path, meta) in own_files(folder, is_kind) {
        let far_on = latest(&meta).is_some_and(|t| t > now + SKEW);
        if far_on || age(&meta, now).is_some_and(|a| a >= old_at) {
            done.remove(&path, meta.len());
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
    let since = match notes_done(done_file) {
        NotesDone::NotYet => None,
        NotesDone::UpTo(since) => Some(since),
        // Not known how far earlier sweeps went, so every note old now counts as looked at: none goes that may have
        // been put back. Notes that grow old from now on go as usual.
        NotesDone::Unreadable => {
            tidied.notes_stay = Some(match write_notes_done(done_file, until) {
                Ok(()) => {
                    format!("{NOTES_DONE_FILE} in the data folder held something else, so the notes old now stay")
                }
                Err(e) => format!("{NOTES_DONE_FILE} in the data folder couldn't be written ({e})"),
            });
            return tidied;
        }
        NotesDone::CantLook => {
            tidied.notes_stay = Some(format!("{NOTES_DONE_FILE} in the data folder couldn't be read"));
            return tidied;
        }
    };
    let old: Vec<&(NaiveDateTime, Vec<(PathBuf, Metadata)>)> =
        notes.values().filter(|(saved, _)| *saved <= until && since.is_none_or(|since| *saved > since)).collect();
    if old.is_empty() {
        return tidied;
    }
    // Nobody is there to answer if Windows asks "permanently delete?", and a yes would remove them for good.
    if let Some(why) = bin.refuses_quietly(folder, 0) {
        tidied.notes_stay = Some(why);
        return tidied;
    }
    // Written before any note goes, so each is looked at once: one put back from the bin, or one the bin wouldn't take,
    // stays, and is never tried again.
    if let Err(e) = write_notes_done(done_file, until) {
        tidied.notes_stay = Some(format!("{NOTES_DONE_FILE} in the data folder couldn't be written ({e})"));
        return tidied;
    }
    for (saved, parts) in old {
        // The picture first, so that if it can't go, its words stay with it.
        let mut parts: Vec<&(PathBuf, Metadata)> = parts.iter().collect();
        parts.sort_by_key(|(path, _)| path.extension().is_none_or(|end| end != "png"));
        if let Some(why) = parts.iter().find_map(|(path, meta)| bin.refuses_quietly(path, meta.len())) {
            tidied.notes_stay.get_or_insert(why);
            continue;
        }
        for (path, meta) in parts {
            if let Err(why) = tidied.send(bin, path, meta.len()) {
                // The first one the bin won't take ends this sweep's notes. It stays, and isn't tried again; the notes
                // after it wait for the next start, so at most one is tried and fails at each start (a question from
                // Windows included). If that can't be written down, they stay too.
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let rest = if write_notes_done(done_file, *saved).is_ok() {
                    "the notes after it wait for the next start"
                } else {
                    "so do the notes after it"
                };
                tidied.notes_stay =
                    Some(format!("{name} couldn't go to the Recycle Bin ({why}); it stays, and {rest}"));
                return tidied;
            }
        }
    }
    tidied
}

/// How far earlier sweeps went through the F12 notes ([`NOTES_DONE_FILE`]).
enum NotesDone {
    /// No sweep has put a note in the bin yet.
    NotYet,
    /// Every note saved up to then has been looked at.
    UpTo(NaiveDateTime),
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
            let first = text.lines().next().unwrap_or_default().trim();
            NaiveDateTime::parse_from_str(first, NAME_TIME).map_or(NotesDone::Unreadable, NotesDone::UpTo)
        }
        Err(e) if e.kind() == ErrorKind::InvalidData => NotesDone::Unreadable,
        Err(_) => NotesDone::CantLook,
    }
}

fn write_notes_done(path: &Path, up_to: NaiveDateTime) -> std::io::Result<()> {
    let text = format!(
        "{}\nThe app's clean-up (rule 28) has looked at every F12 note saved up to this time; one still in the notes \
         folder stays.\n",
        up_to.format(NAME_TIME)
    );
    fs::write(path, text)
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

/// Windows: when a file was made is enough, as a copy is made anew.
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
/// settings' (`backup.txt.mo-part`, `backup-songs.txt.mo-part`, see [`crate::backup`]), or a side database being made
/// (`presets.db.new-<number>`, `curves.db.new-<number>`, with SQLite's own files beside it; `mo_presets::fixes`).
fn is_cut_short(name: &str) -> bool {
    [SETTINGS_FILE, SONGS_FILE].iter().any(|f| name.strip_prefix(f) == Some(PART_ENDING))
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
    fn session_logs_keep_their_newest_30_and_any_still_being_written() {
        let dir = test_folder("sessions");
        let logs = dir.join("logs");
        let name = |i: u32| format!("rust-session-2026-10-01_12-00-{i:02}.log");
        // Made oldest first, all a moment ago (a crash loop, or other copies of the app starting).
        for i in 0..35 {
            fresh(&logs.join(name(i)));
        }
        let now = SystemTime::now();
        // Each was written in the last 10 minutes, so it may be another copy's, still being written: none goes yet.
        assert_eq!(prune_logs(&logs, None, now + MINUTE * 5), Tidied::default());
        // Later on, only the newest 30 stay, and the log just made always.
        let done = prune_logs(&logs, Some(&logs.join(name(0))), now + MINUTE * 11);
        assert_eq!(done.removed, 5);
        assert_eq!(names(&logs).len(), crate::log::KEPT);
        assert!(logs.join(name(0)).exists(), "the log just made stays, oldest or not");
        assert!((1..=5).all(|i| !logs.join(name(i)).exists()) && logs.join(name(6)).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_log_dated_a_little_ahead_keeps_its_place() {
        let dir = test_folder("ahead-logs");
        let logs = dir.join("logs");
        let name = |i: u32| format!("rust-session-2026-10-01_12-00-{i:02}.log");
        for i in 1..=30 {
            fresh(&logs.join(name(i)));
        }
        let later = SystemTime::now() + DAY;
        // Written by a copy of the app on a PC whose clock is a little fast: it's the newest, not the oldest.
        changed_at(&logs.join(name(0)), later + MINUTE * 30);
        let done = prune_logs(&logs, None, later);
        assert_eq!(done.removed, 1);
        assert!(logs.join(name(0)).exists(), "a little ahead, it keeps its place");
        assert!(!logs.join(name(1)).exists() && logs.join(name(2)).exists(), "the oldest goes");
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
            places.temp.join("note-2026-01-05_10-00-00.md"),
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
            notes.join("old").join("note-2026-07-01_10-00-00.md"),
            places.data.join("note-2026-07-01_10-00-00.md"),
            places.data.join("logs").join("note-2026-07-01_10-00-00.md"),
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

        // A Recycle Bin that holds less than the picture: that note stays whole; the other goes.
        let small =
            PickyBin { bin: TestBin(dir.join("bin")), refuses: |_, bytes| (bytes > 1).then(|| "too big".into()) };
        let done = sweep(&places, now, &small);
        assert_eq!(
            done,
            Tidied { to_bin: 1, to_bin_bytes: 1, notes_stay: Some("too big".to_owned()), ..Tidied::default() }
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
        assert!(done.to_bin == 0 && done.notes_stay.is_some() && note.exists() && done_file.is_dir());
        fs::remove_dir(&done_file).unwrap();

        // Cut short by a crash: how far earlier sweeps went isn't known, so the notes old now stay (one may have been
        // put back), and it's written anew.
        fs::write(&done_file, "2026-0").unwrap();
        let done = sweep(&places, now, &bin);
        assert!(done.to_bin == 0 && done.notes_stay.is_some() && note.exists());
        assert_eq!(fs::read_to_string(&done_file).unwrap().lines().next(), Some("2026-08-01_09-00-00"));
        // Notes that grow old from then on go as usual.
        fresh(&notes.join("note-2026-09-01_10-00-00.md"));
        fresh(&notes.join("note-2026-12-20_10-00-00.md"));
        assert_eq!(sweep(&places, local("2027-01-01_12-00-00"), &bin).to_bin, 1);
        assert!(note.exists() && !notes.join("note-2026-09-01_10-00-00.md").exists());
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

    #[test]
    fn a_copy_keeps_its_old_changed_time_but_is_as_new_as_when_it_was_made() {
        let dir = test_folder("copied");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let bin = TestBin(dir.join("bin"));
        let now = SystemTime::now();
        let list = places.temp.join("mo-admin-1-2-3.txt");
        // As a copy is: made now, last changed long ago.
        changed_at(&list, now - DAY * 2500);
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
            places.data.join("MusicOrganiser-build").join("debug").join("presets.db.new-1"),
            places.data.join("notes").join("old").join("note-2020-01-01_00-00-00.md"),
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
        let named = ["library-copy.db", "x.mo-part-1-2", "backup.txt.mo-part", "note-2000-01-01_00-00-00.md"];
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
        fs::create_dir_all(places.data.join("notes")).unwrap();
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
        let named = ["library-copy.db", "x.mo-part-1-2", "note-2000-01-01_00-00-00.md"];
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
