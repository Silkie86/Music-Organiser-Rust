//! Rule 28 (Solomon, 2026-11-02: "make sure the app cleans up after itself where possible, I don't want the app filling
//! up my PC as I use it"). Every file the app writes for itself has a limit or goes once it's done with, and this
//! sweep, a little after each start, clears what a crash or a stopped tool left behind.
//!
//! It only removes plain files whose whole name the app gives them, right in the folders it writes them to: never a
//! file by its folder alone, never anything in a folder inside those, never through a link (a folder on the way that
//! is a link, or a Windows junction, is passed over, and so is a file that is one), and never anything with another
//! name. What it clears:
//!
//! - `logs\` in the app's data folder: crash logs (`crash-<time>.log`) past the newest 30, and each tool's reports
//!   (the twin test's, the MIDI twin test's, the galaxy check's) past the newest 10 of that kind. The session logs keep
//!   their own newest 30 ([`crate::log`]).
//! - The data folder: a settings or database file whose writing a crash cut short (`backup.txt.mo-part`,
//!   `presets.db.new-<number>`), once it's an hour old, so one being written right now is left alone.
//! - `Curves\`: a curve file whose writing a crash cut short (`… .mo-part-<number>-<number>`), an hour old.
//! - `twin\cs\`: the twin test's copy of your library (`library-copy.db`), left when the test was stopped, a day old.
//! - Your Temp folder: the Space page's lists for the administrator step (`mo-admin-<n>-<n>-<n>.txt`), a day old.
//!
//! A file's age runs from the later of when it was made and when it was last changed: a copy keeps the changed time of
//! what it was copied from, so a file from 2019 copied in today is a day old tomorrow, not seven years old.
//!
//! Never touched: samples (rule 2), projects, "(fixed)" copies (rule 3), converts and exports, F12 notes with their
//! pictures and drafts (rule 17), Playground ideas, settings, the databases (library.db, sounds.db and midi.db are the
//! C# app's too, and opened read-only), the C# app's own logs and reports, the launcher's folders (`run-latest-rust\`
//! and its build folders, which it keeps in bounds itself), and everything else in those folders.
//!
//! **A feature that writes temp files later** (a MIDI clip dragged out to a DAW) gives each a name of its own: its own
//! start, the app's process id and a count, such as `mo-clip-<pid>-<n>.mid`, right in the Temp folder (not in a folder
//! of its own: a folder's name proves nothing, since the system or the user can point Temp anywhere, even at the home
//! folder). It removes each one when it's done with it, and adds that exact name here, cleared at the top of the Temp
//! folder only, by the later of when it was made and changed. Never a whole folder by its name or by age alone.

use std::fs::{self, Metadata};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::backup::{SETTINGS_FILE, SONGS_FILE};
use crate::log::is_crash_log;
use crate::paths;

/// Crash logs kept: the newest 30, as for the session logs.
pub const CRASH_LOGS_KEPT: usize = 30;

/// Reports kept of each kind.
pub const REPORTS_KEPT: usize = 10;

/// The tools' reports in `logs\`, by how their names start. Each name is this, then the time it ran (digits, `-` and
/// `_`), then `.txt`: `twin-test-2026-11-02-1430.txt`. Written by `tools/dev/twin-test.ps1`, `tools/dev/MIDI twin
/// test.cmd` and the galaxy check (`tools/galaxycheck`); the C# app writes none of these.
pub const REPORTS: [&str; 6] =
    ["twin-test-", "twin-report-", "twin-songs-", "midi-twin-", "midi-twin-report-", "galaxy-check-"];

/// The folder in the data folder that curve files are written to (`mo_presets::curves::FOLDER`).
const CURVES: &str = "Curves";

/// The twin test's copy of the library, beside its C# helper in `twin\cs\` (`tools/twin`, `run_songs`).
const LIBRARY_COPY: &str = "library-copy.db";

/// How long a file being written may take before what's left of it counts as a crash's leftover.
const WRITING: Duration = Duration::from_secs(60 * 60);
pub(crate) const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// How far after now a file's time may be and still count as just made: the clock was a little fast when it was
/// written, or was set back since. Further on than this, the clock was plainly wrong once, and the time counts for
/// nothing (the file is as old as can be).
const SKEW: Duration = DAY;
/// Logs (session and crash) younger than this are never removed to keep the count down: one may be another copy of
/// the app's, still being written, or a crash that just happened.
pub(crate) const YOUNG: Duration = DAY;

/// How long after start the sweep waits, so it never competes with the first page for the disk (rule 12).
const WAIT: Duration = Duration::from_secs(20);

/// What a sweep removed, and what it couldn't (in use, or read-only), which is tried again next time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tidied {
    pub removed: usize,
    pub bytes: u64,
    pub failed: usize,
}

impl Tidied {
    fn add(&mut self, other: Tidied) {
        self.removed += other.removed;
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.failed += other.failed;
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
        report(sweep(&Places::this_pc(), SystemTime::now()));
    });
    if let Err(e) = started {
        crate::log::warning("Tidy up", &format!("Couldn't start clearing the app's leftovers: {e}"));
    }
}

fn report(done: Tidied) {
    if done.removed > 0 {
        let files = if done.removed == 1 { "1 file".to_owned() } else { format!("{} files", done.removed) };
        let size = mo_space::size_text(done.bytes);
        crate::log::info("Tidy up", &format!("Cleared {files} the app had left behind ({size})"));
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

/// Clears what the app left in `places` (see the top of this file), as it is at `now`. Reads and removes files, so
/// never on the window's thread.
pub fn sweep(places: &Places, now: SystemTime) -> Tidied {
    let mut done = Tidied::default();
    if let Some(logs) = real_folder(&places.data, &["logs"]) {
        // The folder is listed once, then each kind keeps its newest.
        let all = own_files(&logs, |n| is_crash_log(n) || REPORTS.iter().any(|start| is_report(n, start)));
        let of = |is_kind: &dyn Fn(&str) -> bool| -> Vec<(PathBuf, Metadata)> {
            all.iter().filter(|(p, _)| p.file_name().and_then(|n| n.to_str()).is_some_and(is_kind)).cloned().collect()
        };
        done.add(keep_newest_of(of(&is_crash_log), CRASH_LOGS_KEPT, now, None, YOUNG));
        for start in REPORTS {
            done.add(keep_newest_of(of(&|n| is_report(n, start)), REPORTS_KEPT, now, None, Duration::ZERO));
        }
    }
    done.add(remove_older(&places.data, is_cut_short, WRITING, now));
    if let Some(curves) = real_folder(&places.data, &[CURVES]) {
        done.add(remove_older(&curves, is_curve_cut_short, WRITING, now));
    }
    if let Some(twin) = real_folder(&places.data, &["twin", "cs"]) {
        done.add(remove_older(&twin, is_library_copy, DAY, now));
    }
    done.add(remove_older(&places.temp, mo_space::admin::is_temp_file, DAY, now));
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

/// Deletes all but the newest `keep` of the plain files right in `folder` whose names `is_kind` knows (see
/// [`keep_newest_of`]). Nothing else in the folder is looked at, and nothing in a folder that is a link or a junction.
pub(crate) fn keep_newest(
    folder: &Path,
    is_kind: impl Fn(&str) -> bool,
    keep: usize,
    now: SystemTime,
    spare: Option<&Path>,
    young: Duration,
) -> Tidied {
    if !fs::symlink_metadata(folder).is_ok_and(|m| m.is_dir()) {
        return Tidied::default();
    }
    keep_newest_of(own_files(folder, is_kind), keep, now, spare, young)
}

/// Deletes all but the newest `keep` of `files`, newest by the later of when each was made and last changed (see
/// [`age`]). A time up to [`SKEW`] after `now` counts as now; one further on (a clock that was plainly wrong once)
/// would hold its place for ever, so it counts as the oldest. `spare`, the file just made, is never deleted and
/// counts as one of the newest, and neither is a file younger than `young`.
fn keep_newest_of(
    mut files: Vec<(PathBuf, Metadata)>,
    keep: usize,
    now: SystemTime,
    spare: Option<&Path>,
    young: Duration,
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
        let is_young =
            latest(meta).is_some_and(|t| t <= now + SKEW && now.duration_since(t).unwrap_or_default() < young);
        if !is_young {
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

/// How old a file is at `now`: from the later of when it was made and when it was last changed (a copy keeps the
/// changed time of the file it was copied from). `None` when neither can be read, or the later one is after `now`.
fn age(meta: &Metadata, now: SystemTime) -> Option<Duration> {
    now.duration_since(latest(meta)?).ok()
}

/// The later of when a file was made and when it was last changed, when either can be read.
fn latest(meta: &Metadata) -> Option<SystemTime> {
    match (meta.created().ok(), meta.modified().ok()) {
        (Some(made), Some(changed)) => Some(made.max(changed)),
        (made, changed) => made.or(changed),
    }
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

/// A file in the data folder whose writing was cut short: the Backup settings' (`backup.txt.mo-part`,
/// `backup-songs.txt.mo-part`, see [`crate::backup`]), or a side database being made (`presets.db.new-<number>`,
/// `curves.db.new-<number>`, with SQLite's own files beside it; `mo_presets::fixes::make_new`).
fn is_cut_short(name: &str) -> bool {
    if [SETTINGS_FILE, SONGS_FILE].iter().any(|f| name.strip_prefix(f) == Some(".mo-part")) {
        return true;
    }
    ["presets.db.new-", "curves.db.new-"].iter().any(|start| {
        name.strip_prefix(start).is_some_and(|rest| {
            let number = ["-journal", "-wal", "-shm"].iter().find_map(|end| rest.strip_suffix(end)).unwrap_or(rest);
            is_number(number)
        })
    })
}

/// A curve file whose writing was cut short: its name, `.mo-part-`, then two numbers (`mo_presets::export`).
fn is_curve_cut_short(name: &str) -> bool {
    let Some((before, numbers)) = name.rsplit_once(".mo-part-") else { return false };
    !before.is_empty() && numbers.split_once('-').is_some_and(|(a, b)| is_number(a) && is_number(b))
}

/// The twin test's copy of the library, or SQLite's own files beside it.
fn is_library_copy(name: &str) -> bool {
    name.strip_prefix(LIBRARY_COPY).is_some_and(|end| ["", "-wal", "-shm", "-journal"].contains(&end))
}

fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use std::fs::File;

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

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn crash_logs_and_reports_keep_their_newest_and_nothing_else_goes() {
        let dir = test_folder("logs");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let logs = places.data.join("logs");
        let now = SystemTime::now();
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

        // Crash logs under a day old are never removed (another copy's, or a crash just now), so three days on.
        let done = sweep(&places, now + DAY * 3);
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
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn leftovers_go_once_old_enough_and_the_users_files_never() {
        let dir = test_folder("leftovers");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
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
            data.join("notes").join("note-2026-10-01_12-00-00.md"),
            data.join("notes").join("note-2026-10-01_12-00-00.png"),
            data.join("notes").join("draft.txt"),
            data.join("notes").join("unsaved.txt"),
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
            places.temp.join("MusicOrganiser-Rust").join("Bass line.mid"),
        ];
        for path in writes.iter().chain(&days).chain(&kept) {
            fresh(path);
        }
        // Ages run from when each was made, so the sweep is run as if later on.
        let made = SystemTime::now();
        assert_eq!(sweep(&places, made + MINUTE * 10), Tidied::default(), "all too new");
        let done = sweep(&places, made + MINUTE * 61);
        assert_eq!(done, Tidied { removed: writes.len(), bytes: writes.len() as u64, failed: 0 });
        assert!(writes.iter().all(|p| !p.exists()) && days.iter().all(|p| p.exists()), "only the hour's go");
        let done = sweep(&places, made + DAY + MINUTE);
        assert_eq!(done, Tidied { removed: days.len(), bytes: days.len() as u64, failed: 0 });
        for path in writes.iter().chain(&days) {
            assert!(!path.exists(), "{} goes", path.display());
        }
        assert_eq!(sweep(&places, made + DAY * 4000), Tidied::default());
        for path in &kept {
            assert!(path.exists(), "{} stays", path.display());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_copy_keeps_its_old_changed_time_but_is_as_old_as_when_it_was_made() {
        let dir = test_folder("copied");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let now = SystemTime::now();
        let list = places.temp.join("mo-admin-1-2-3.txt");
        // As a copy is: made now, last changed long ago.
        changed_at(&list, now - DAY * 2500);
        let made_is_known = fs::metadata(&list).unwrap().created().is_ok();
        let done = sweep(&places, now + MINUTE);
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
        // Files named as the sweep clears them, but inside folders: the launcher's, beside the code, and wherever
        // -BuildDir put a build folder (the data folder, its logs, the Temp folder), and a folder the app might once
        // have used in Temp.
        let inside = [
            places.data.join("run-latest-rust").join("run").join("20261101-120000").join("music-organiser.exe"),
            places.data.join("run-latest-rust").join("app").join("target").join("debug").join("crash-1.log"),
            places.data.join("run-latest-rust").join("backup.txt.mo-part"),
            places.data.join("MusicOrganiser-build").join("debug").join("presets.db.new-1"),
            places.data.join("logs").join("MusicOrganiser-build").join("crash-1700000000.log"),
            places.data.join("twin").join("backup.txt.mo-part"),
            places.temp.join("MusicOrganiser-build").join("mo-admin-1-2-3.txt"),
            places.temp.join("MusicOrganiser-Rust").join("mo-admin-1-2-3.txt"),
        ];
        for path in &inside {
            fresh(path);
        }
        // Something of the sweep's own beside them, so it's seen to run.
        fresh(&places.temp.join("mo-admin-9-9-9.txt"));

        let done = sweep(&places, SystemTime::now() + DAY * 4000);
        assert_eq!(done, Tidied { removed: 1, bytes: 1, failed: 0 });
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
        let yours = dir.join("yours");
        let song = yours.join("Song.flp");
        fresh(&song);
        for name in ["library-copy.db", "x.mo-part-1-2", "backup.txt.mo-part"] {
            fresh(&yours.join("cs").join(name));
            fresh(&yours.join(name));
        }
        fs::create_dir_all(places.data.join("logs")).unwrap();
        fs::create_dir_all(&places.temp).unwrap();
        symlink(&song, places.data.join("logs").join("crash-1700000000.log")).unwrap();
        for i in 0..40 {
            symlink(&song, places.data.join("logs").join(format!("twin-test-2026-10-01-{i:04}.txt"))).unwrap();
        }
        symlink(&song, places.data.join("backup.txt.mo-part")).unwrap();
        symlink(&song, places.temp.join("mo-admin-1-2-3.txt")).unwrap();
        symlink(&yours, places.data.join("twin")).unwrap();
        symlink(&yours, places.data.join(CURVES)).unwrap();

        assert_eq!(sweep(&places, SystemTime::now() + DAY * 4000), Tidied::default());
        assert!(song.exists());
        for name in ["library-copy.db", "x.mo-part-1-2", "backup.txt.mo-part"] {
            assert!(yours.join("cs").join(name).exists() && yours.join(name).exists(), "{name}");
        }
        assert!(fs::symlink_metadata(places.temp.join("mo-admin-1-2-3.txt")).is_ok(), "the link itself stays too");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The same with Windows junctions (`mklink /J`, which needs no administrator rights).
    #[cfg(windows)]
    #[test]
    fn nothing_is_removed_through_a_junction() {
        let dir = test_folder("junctions");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let yours = dir.join("yours");
        let junction = |at: &Path, to: &Path| {
            let made = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(at).arg(to).output();
            assert!(made.is_ok_and(|o| o.status.success()), "mklink /J {}", at.display());
        };
        for name in ["library-copy.db", "x.mo-part-1-2"] {
            fresh(&yours.join("cs").join(name));
            fresh(&yours.join(name));
        }
        fs::create_dir_all(places.data.join("logs")).unwrap();
        fs::create_dir_all(&places.temp).unwrap();
        junction(&places.data.join("twin"), &yours);
        junction(&places.data.join(CURVES), &yours);
        junction(&places.data.join("logs").join("crash-1700000000.log"), &yours);
        junction(&places.temp.join("mo-admin-1-2-3.txt"), &yours);

        assert_eq!(sweep(&places, SystemTime::now() + DAY * 4000), Tidied::default());
        for name in ["library-copy.db", "x.mo-part-1-2"] {
            assert!(yours.join("cs").join(name).exists() && yours.join(name).exists(), "{name}");
        }
        assert!(fs::symlink_metadata(places.temp.join("mo-admin-1-2-3.txt")).is_ok(), "the junction itself stays too");
        // Junctions first, so removing the test folder never reaches through them.
        for at in [
            places.data.join("twin"),
            places.data.join(CURVES),
            places.data.join("logs").join("crash-1700000000.log"),
            places.temp.join("mo-admin-1-2-3.txt"),
        ] {
            let _ = fs::remove_dir(at);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_there_is_nothing_to_do() {
        let dir = test_folder("empty");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        assert_eq!(sweep(&places, SystemTime::now()), Tidied::default());
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
        assert!(!is_cut_short("presets.db.new-12-old") && !is_cut_short("presets.db") && !is_cut_short("backup.txt"));
        assert!(!is_cut_short("library.db.new-12") && !is_cut_short("backup.txt.mo-part.2"));
        assert!(is_curve_cut_short("A.bwcurve.mo-part-1-0") && !is_curve_cut_short("A.bwcurve.mo-part-1-x"));
        assert!(!is_curve_cut_short("A.bwcurve.mo-part-1") && !is_curve_cut_short("A.bwcurve"));
        assert!(is_library_copy("library-copy.db-shm") && !is_library_copy("library-copy.db2"));
    }
}
