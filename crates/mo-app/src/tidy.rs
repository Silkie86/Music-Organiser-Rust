//! Rule 28 (Solomon, 2026-11-02: "make sure the app cleans up after itself where possible, I don't want the app filling
//! up my PC as I use it"). Every file the app writes for itself has a limit or goes once it's done with, and this
//! sweep, a little after each start, clears what a crash or a stopped tool left behind.
//!
//! It only removes plain files it can tell the app made, by the folder they're in and their whole name, never by a
//! guess: never a folder of yours, never through a link, and never anything with another name. What it clears:
//!
//! - `logs\` in the app's data folder: crash logs (`crash-<time>.log`) past the newest 30, and each tool's reports
//!   (the twin test's, the MIDI twin test's, the galaxy check's) past the newest 10 of that kind. The session logs keep
//!   their own newest 30 ([`crate::log`]).
//! - The data folder: a settings or database file whose writing a crash cut short (`backup.txt.mo-part`,
//!   `presets.db.new-<number>`), once it's an hour old, so one being written right now is left alone.
//! - `Curves\`: a curve file whose writing a crash cut short (`… .mo-part-<number>-<number>`), an hour old.
//! - `twin\cs\`: the twin test's copy of your library (`library-copy.db`), left when the test was stopped, a day old.
//! - Your Temp folder: the Space page's lists for the administrator step (`mo-admin-<n>-<n>-<n>.txt`), a day old.
//! - The app's own folder in your Temp folder ([`crate::paths::temp_dir`]): anything that's been there a week.
//!
//! Never touched: samples (rule 2), projects, "(fixed)" copies (rule 3), converts and exports, F12 notes with their
//! pictures and drafts (rule 17), Playground ideas, settings, the databases (library.db, sounds.db and midi.db are the
//! C# app's too, and opened read-only), the C# app's own logs and reports, and everything else in those folders.
//!
//! Nor the launcher's folders: `run-latest-rust\` and its build folders (beside the code, or a `MusicOrganiser-build`
//! wherever `-BuildDir` puts it) are the launcher's to keep in bounds (`tools/dev/run-latest.ps1`). The sweep never
//! goes into a folder inside the data folder at all, and in its own temp folder it never goes into a folder called
//! either of those names, nor any folder cargo marked as a build folder of its own (it holds a `CACHEDIR.TAG`).

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
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const WEEK: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How long after start the sweep waits, so it never competes with the first page for the disk (rule 12).
const WAIT: Duration = Duration::from_secs(20);

/// How deep the sweep goes in the app's own temp folder.
const DEEPEST: usize = 8;

/// The launcher's own folders (`tools/dev/run-latest.ps1`): its home in the data folder, and the build folder it makes
/// inside the folder `-BuildDir` names. The sweep never goes into a folder with one of these names.
pub const LAUNCHER_FOLDERS: [&str; 2] = ["run-latest-rust", "MusicOrganiser-build"];

/// The file cargo puts in a build folder it made (and other programs in caches they keep): such a folder is theirs.
const CACHE_TAG: &str = "CACHEDIR.TAG";

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
    let logs = places.data.join("logs");
    done.add(keep_newest(&logs, is_crash_log, CRASH_LOGS_KEPT));
    for start in REPORTS {
        done.add(keep_newest(&logs, |n| is_report(n, start), REPORTS_KEPT));
    }
    done.add(remove_older(&places.data, is_cut_short, WRITING, now));
    done.add(remove_older(&places.data.join(CURVES), is_curve_cut_short, WRITING, now));
    done.add(remove_older(&places.data.join("twin").join("cs"), is_library_copy, DAY, now));
    done.add(remove_older(&places.temp, mo_space::admin::is_temp_file, DAY, now));
    done.add(clear_own_folder(&places.temp.join(paths::TEMP_FOLDER), WEEK, now));
    done
}

/// Deletes all but the newest `keep` of the plain files in `folder` whose names `is_kind` knows, newest by when they
/// were last written. Nothing else in the folder is looked at.
pub(crate) fn keep_newest(folder: &Path, is_kind: impl Fn(&str) -> bool, keep: usize) -> Tidied {
    let mut done = Tidied::default();
    let mut files = own_files(folder, is_kind);
    if files.len() <= keep {
        return done;
    }
    // Names alone put "…-05-2.log" before "…-05.log", so the time each was last written decides.
    files.sort_by_cached_key(|(p, m)| (m.modified().ok(), p.clone()));
    for (old, meta) in &files[..files.len() - keep] {
        done.remove(old, meta.len());
    }
    done
}

/// Deletes the plain files in `folder` whose names `is_kind` knows, once they were last written at least `age` before
/// `now`. One whose time can't be read, or is after `now`, stays.
fn remove_older(folder: &Path, is_kind: impl Fn(&str) -> bool, age: Duration, now: SystemTime) -> Tidied {
    let mut done = Tidied::default();
    for (path, meta) in own_files(folder, is_kind) {
        if is_old(&meta, age, now) {
            done.remove(&path, meta.len());
        }
    }
    done
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

fn is_old(meta: &Metadata, age: Duration, now: SystemTime) -> bool {
    meta.modified().ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|d| d >= age)
}

/// Deletes everything in the app's own temp folder that was last written at least `age` before `now`, then the
/// folders inside it that are empty by then and had nothing new put in them for as long. The folder must really be
/// there, not a link to somewhere else; and links inside it are only ever left as they are, as are the launcher's
/// folders and cargo's build folders ([`LAUNCHER_FOLDERS`], [`CACHE_TAG`]).
fn clear_own_folder(folder: &Path, age: Duration, now: SystemTime) -> Tidied {
    let mut done = Tidied::default();
    if fs::symlink_metadata(folder).is_ok_and(|m| m.is_dir()) {
        clear_inside(folder, age, now, 0, &mut done);
    }
    done
}

/// Whether a folder is another program's to keep: one cargo made as a build folder (or another program as a cache),
/// marked with a [`CACHE_TAG`] file.
fn is_marked_cache(folder: &Path) -> bool {
    fs::symlink_metadata(folder.join(CACHE_TAG)).is_ok()
}

/// Whether a folder's name is one of the launcher's own ([`LAUNCHER_FOLDERS`]), letter case aside.
fn is_launchers(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    LAUNCHER_FOLDERS.iter().any(|l| l.eq_ignore_ascii_case(&name))
}

fn clear_inside(folder: &Path, age: Duration, now: SystemTime, depth: usize, done: &mut Tidied) {
    if is_marked_cache(folder) {
        return;
    }
    let Ok(entries) = fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        // From the listing: a link (to a file or a folder, or a Windows junction) is neither a file nor a folder here.
        let (Ok(kind), Ok(meta)) = (entry.file_type(), entry.metadata()) else { continue };
        let path = entry.path();
        if kind.is_file() && is_old(&meta, age, now) {
            done.remove(&path, meta.len());
        } else if kind.is_dir() && depth < DEEPEST && !is_launchers(&entry.file_name()) {
            // Whether anything was put in it lately, before its old files go (that changes its time).
            let quiet = is_old(&meta, age, now);
            clear_inside(&path, age, now, depth + 1, done);
            if quiet {
                // Only an empty folder can be removed this way: one with anything left in it stays.
                let _ = fs::remove_dir(&path);
            }
        }
    }
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

    /// Writes `path` (and its folders), last changed `ago` before `now`.
    fn file(path: &Path, ago: Duration, now: SystemTime) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
        File::options().write(true).open(path).unwrap().set_modified(now - ago).unwrap();
    }

    /// Sets when a folder was last changed.
    fn folder_time(folder: &Path, when: SystemTime) {
        let mut options = File::options();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Leave to change its times (FILE_WRITE_ATTRIBUTES), and the flag without which Windows won't open a folder
            // (FILE_FLAG_BACKUP_SEMANTICS).
            options.access_mode(0x100).custom_flags(0x0200_0000);
        }
        #[cfg(not(windows))]
        options.read(true);
        options.open(folder).unwrap().set_modified(when).unwrap();
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
            file(&logs.join(format!("crash-{}.log", 1_700_000_000 + i)), MINUTE * (100 - i), now);
        }
        for i in 0..12u32 {
            file(&logs.join(format!("twin-report-2026-10-{:02}-1200.txt", i + 1)), MINUTE * (50 - i), now);
            file(&logs.join(format!("midi-twin-2026-10-{:02}-1200.txt", i + 1)), MINUTE * (50 - i), now);
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
            file(&logs.join(name), DAY * 400, now);
        }
        // A folder named like a crash log is never removed.
        fs::create_dir_all(logs.join("crash-1600000000.log")).unwrap();

        let done = sweep(&places, now);
        assert_eq!(done.removed, 5 + 2 + 2);
        assert_eq!(done.failed, 0);
        let left = names(&logs);
        assert_eq!(left.iter().filter(|n| is_crash_log(n)).count(), CRASH_LOGS_KEPT + 1, "30 files and the folder");
        assert!(
            left.contains(&"crash-1700000034.log".to_owned()) && !left.contains(&"crash-1700000004.log".to_owned())
        );
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
        let now = SystemTime::now();
        let data = &places.data;
        let old = [
            data.join("backup.txt.mo-part"),
            data.join("backup-songs.txt.mo-part"),
            data.join("presets.db.new-4120"),
            data.join("curves.db.new-77-journal"),
            data.join("Curves").join("Rise abcd1234.bwcurve.mo-part-4120-3"),
            data.join("twin").join("cs").join("library-copy.db"),
            data.join("twin").join("cs").join("library-copy.db-wal"),
            places.temp.join("mo-admin-4120-1730000000000000000-0.txt"),
            places.temp.join("mo-admin-4120-1730000000000000000-0-result.txt"),
        ];
        for path in &old {
            file(path, DAY * 2, now);
        }
        // Too new: may be being written or used right now (an hour for a write, a day for the rest).
        file(&data.join("presets.db.new-9"), MINUTE * 10, now);
        file(&data.join("Curves").join("Fall 0000ffff.bwcurve.mo-part-9-1"), MINUTE * 10, now);
        file(&places.temp.join("mo-admin-9-9-9.txt"), Duration::from_secs(60 * 60 * 20), now);
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
        ];
        for path in &kept {
            file(path, DAY * 400, now);
        }

        let done = sweep(&places, now);
        assert_eq!(done, Tidied { removed: old.len(), bytes: old.len() as u64, failed: 0 });
        for path in &old {
            assert!(!path.exists(), "{} goes", path.display());
        }
        for path in &kept {
            assert!(path.exists(), "{} stays", path.display());
        }
        assert!(data.join("presets.db.new-9").exists(), "an hour hasn't passed");
        assert!(data.join("Curves").join("Fall 0000ffff.bwcurve.mo-part-9-1").exists());
        assert!(places.temp.join("mo-admin-9-9-9.txt").exists(), "a day hasn't passed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_apps_own_temp_folder_keeps_a_weeks_files_and_leaves_links_alone() {
        let dir = test_folder("temp");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let own = places.temp.join(paths::TEMP_FOLDER);
        let now = SystemTime::now();
        file(&own.join("Bass line.mid"), DAY * 8, now);
        file(&own.join("drag 1").join("Chords.mid"), DAY * 8, now);
        file(&own.join("Lead.mid"), DAY * 2, now);
        file(&own.join("drag 2").join("Pad.mid"), DAY * 2, now);
        // Outside the app's own folder, in the temp folder itself: not the app's, so it stays.
        file(&places.temp.join("Bass line.mid"), DAY * 400, now);
        let elsewhere = dir.join("yours");
        file(&elsewhere.join("Song.flp"), DAY * 400, now);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&elsewhere, own.join("a link")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(elsewhere.join("Song.flp"), own.join("a file link.flp")).unwrap();
        // Folders' own times, once their files are in: one quiet for a week, one with a file put in lately.
        folder_time(&own.join("drag 1"), now - DAY * 8);

        let done = sweep(&places, now);
        assert_eq!(done.removed, 2);
        assert!(!own.join("Bass line.mid").exists() && !own.join("drag 1").exists(), "a week old, and emptied");
        assert!(own.join("Lead.mid").exists() && own.join("drag 2").join("Pad.mid").exists(), "newer stays");
        assert!(places.temp.join("Bass line.mid").exists());
        assert!(elsewhere.join("Song.flp").exists(), "nothing is removed through a link");
        #[cfg(unix)]
        assert!(
            fs::symlink_metadata(own.join("a link")).is_ok()
                && fs::symlink_metadata(own.join("a file link.flp")).is_ok()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_launchers_folders_and_cargos_build_folders_are_never_gone_into() {
        let dir = test_folder("launcher");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let own = places.temp.join(paths::TEMP_FOLDER);
        let now = SystemTime::now();
        let old = DAY * 400;
        // Files named as the sweep clears them elsewhere, inside the launcher's folders: beside the code, and wherever
        // -BuildDir put a build folder (the data folder, its logs, the Temp folder, the app's own temp folder).
        let launchers = [
            places.data.join("run-latest-rust").join("run").join("20261101-120000").join("music-organiser.exe"),
            places.data.join("run-latest-rust").join("app").join("target").join("debug").join("crash-1.log"),
            places.data.join("run-latest-rust").join("backup.txt.mo-part"),
            places.data.join("MusicOrganiser-build").join("debug").join("presets.db.new-1"),
            places.data.join("logs").join("MusicOrganiser-build").join("crash-1700000000.log"),
            places.temp.join("MusicOrganiser-build").join("mo-admin-1-2-3.txt"),
            own.join("MusicOrganiser-build").join("debug").join("deps").join("libmo_app.rlib"),
            own.join("musicorganiser-BUILD").join("CACHEDIR.TAG"),
            own.join("Run-Latest-Rust").join("last-branch.txt"),
            // A build folder cargo made under another name: its CACHEDIR.TAG says so.
            own.join("Builds").join("target").join("CACHEDIR.TAG"),
            own.join("Builds").join("target").join("debug").join("music-organiser"),
        ];
        for path in &launchers {
            file(path, old, now);
        }
        // Something of the sweep's own beside them, so it's seen to run.
        file(&own.join("Bass line.mid"), old, now);

        let done = sweep(&places, now);
        assert_eq!(done, Tidied { removed: 1, bytes: 1, failed: 0 });
        assert!(!own.join("Bass line.mid").exists());
        for path in &launchers {
            assert!(path.exists(), "{} stays", path.display());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn an_own_temp_folder_that_is_a_link_is_left_alone() {
        let dir = test_folder("linked");
        let places = Places { data: dir.join("data"), temp: dir.join("temp") };
        let now = SystemTime::now();
        let yours = dir.join("yours");
        file(&yours.join("Old song.flp"), DAY * 400, now);
        fs::create_dir_all(&places.temp).unwrap();
        std::os::unix::fs::symlink(&yours, places.temp.join(paths::TEMP_FOLDER)).unwrap();
        assert_eq!(sweep(&places, now), Tidied::default());
        assert!(yours.join("Old song.flp").exists());
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
