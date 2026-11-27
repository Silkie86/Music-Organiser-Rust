//! Where the app keeps its files. At first that's the system's own place for an app's data
//! (`%LOCALAPPDATA%\MusicOrganiser` on Windows, the folder the C# app used too). Settings can move it to another drive
//! ([`crate::data_move`], Solomon 2026-11-05: C: was full and slow): a small file beside that first folder,
//! [`POINTER_FILE`], then names the folder the app uses. It's read once, the first time [`data_dir`] is asked, so the
//! whole run uses one folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Beside the first data folder (`%LOCALAPPDATA%\MusicOrganiser-folder.txt`), not in it, so removing the old folder
/// once a move is done never loses where the data went. One line: the folder the app now uses.
pub const POINTER_FILE: &str = "MusicOrganiser-folder.txt";

/// In a folder the data moved into, from the moment of the switch: says where from and when. A pointer to a folder
/// without it (or the installer's [`crate::install_choice::STARTED_HERE_FILE`]) isn't followed (that folder isn't a finished move: a drive letter given to another disk, a folder made
/// by hand).
pub const MOVED_IN_FILE: &str = "moved-in.json";

/// The folder the data is in and, when a move's pointer couldn't be followed, why (for the log and Settings).
pub(crate) struct Resolved {
    pub(crate) folder: PathBuf,
    pub(crate) problem: Option<String>,
    /// The folder a finished move went to, when the problem is that it isn't there now (its drive is missing, say),
    /// rather than a pointer that can't be read.
    pub(crate) missing: Option<PathBuf>,
}

static RESOLVED: OnceLock<Resolved> = OnceLock::new();

fn resolved() -> &'static Resolved {
    RESOLVED.get_or_init(|| resolve_in(&first_dir()))
}

/// The app's data folder: the one a move went to, or the first one. The same for the whole run. Falls back to a folder
/// beside the current one if the system gives no home at all.
pub fn data_dir() -> PathBuf {
    resolved().folder.clone()
}

/// Why a move's folder isn't the one in use this run (its drive missing, say), so the app's changes now go to the
/// first folder: shown in the log and on Settings. `None` when all is well.
pub fn problem() -> Option<String> {
    resolved().problem.clone()
}

/// Why the folder a move went to isn't there, read now (the run's own folder isn't fixed by it): at start, so the app
/// can ask what to do before anything opens the old copy (F-2). `None` when all is well, and when the trouble is only
/// a pointer that can't be read (that's logged, through [`problem`]: no plugging in fixes it).
pub fn missing_now() -> Option<String> {
    let now = resolve_in(&first_dir());
    now.problem.filter(|_| now.missing.is_some())
}

/// The folder the data moved to that couldn't be found as this run started, so the run is on the old copy: the real
/// data, which a copy made this run would leave behind (DA-2). This run's, not read again (no wait on a drive), as
/// the copy is of this run's folder. `None` when it was there, or there was no move.
pub fn missing_folder() -> Option<PathBuf> {
    resolved().missing.clone()
}

/// Whether the first data folder holds anything of the app's (anything but the launchers' own folders and the logs):
/// when it holds nothing, opening on it while the data folder's drive is missing opens with no data at all, as on a
/// PC the installer started the data elsewhere (DA-4).
pub fn first_has_data() -> bool {
    holds_data(&first_dir())
}

/// Whether `folder` holds anything but the launchers' own folders, the logs and the app's lock files (the helper writes
/// its log there at a login while the data's drive is missing, and a run opened with no data writes its log and holds
/// its lock there: the whole-app review of 26 Nov, BD-3).
fn holds_data(folder: &Path) -> bool {
    fs::read_dir(folder).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            let name = e.file_name();
            let skipped = crate::data_move::LAUNCHER_FOLDERS.iter().any(|l| name.eq_ignore_ascii_case(l))
                || crate::data_move::RUNNING_LOCKS.iter().any(|l| name.eq_ignore_ascii_case(l));
            !(skipped || name.eq_ignore_ascii_case("logs"))
        })
    })
}

/// Whether the first data folder is the old copy a move left: the data moved from it to another folder, and the move
/// left its note there ([`crate::data_move::MOVED_OUT_FILE`]). Only then is opening on it while the moved folder's
/// drive is missing opening on "the old copy from before the move". The whole-app review of 26 Nov, BD-3: after one
/// run opened with no data, the box called the folder that run had written in "the old copy", though no move was
/// ever made from it.
pub fn first_is_old_copy() -> bool {
    is_old_copy(&first_dir())
}

fn is_old_copy(folder: &Path) -> bool {
    fs::symlink_metadata(folder.join(crate::data_move::MOVED_OUT_FILE)).is_ok_and(|m| m.is_file())
}

/// The system's own place for the app's data, where it starts out and where [`POINTER_FILE`] sits beside it.
pub fn first_dir() -> PathBuf {
    let base = if cfg!(windows) {
        folder_var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        folder_var("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        folder_var("XDG_DATA_HOME").or_else(|| folder_var("HOME").map(|h| h.join(".local").join("share")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("MusicOrganiser")
}

/// Where [`POINTER_FILE`] sits for the first data folder `first`.
pub fn pointer_file(first: &Path) -> PathBuf {
    first.with_file_name(POINTER_FILE)
}

/// The folder in use for the first data folder `first`: the pointer's, when it names a folder a move finished in,
/// else `first`. Never cached: [`crate::data_move`] asks before and after a switch.
pub(crate) fn resolve_in(first: &Path) -> Resolved {
    let fallback = |problem: Option<String>| Resolved { folder: first.to_path_buf(), problem, missing: None };
    let text = match fs::read_to_string(pointer_file(first)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return fallback(None),
        Err(e) => return fallback(Some(format!("Couldn't read {POINTER_FILE}: {e}. Using {}", first.display()))),
    };
    // The installer writes it as UTF-8, which may start with a byte-order mark (crate::install_choice).
    let Some(named) = absolute(Some(PathBuf::from(text.trim_start_matches('\u{feff}').trim()))) else {
        return fallback(Some(format!("{POINTER_FILE} doesn't name a whole folder path. Using {}", first.display())));
    };
    // A finished move, or a folder the installer started the data in (crate::install_choice).
    if named.join(MOVED_IN_FILE).is_file() || named.join(crate::install_choice::STARTED_HERE_FILE).is_file() {
        Resolved { folder: named, problem: None, missing: None }
    } else {
        Resolved {
            missing: Some(named.clone()),
            ..fallback(Some(format!(
                "The data folder {} can't be found (is its drive there?). Until it's back, the app uses {}, and \
                 changes made meanwhile stay there",
                named.display(),
                first.display()
            )))
        }
    }
}

/// The folder in use for the first data folder `first`, read now (not the run's own, which [`data_dir`] keeps).
#[cfg(test)]
pub(crate) fn folder_in_use(first: &Path) -> PathBuf {
    resolve_in(first).folder
}

/// The folder the data was moved from, when the folder in use is a move's (read from its [`MOVED_IN_FILE`]): where
/// the old copies stay until you remove them.
pub fn moved_from() -> Option<PathBuf> {
    static FROM: OnceLock<Option<PathBuf>> = OnceLock::new();
    FROM.get_or_init(|| crate::data_move::moved_in(&data_dir()).map(|m| PathBuf::from(m.from))).clone()
}

/// The folder an earlier move went to that the move to the folder in use left behind, as you said yes to (it couldn't
/// be found then, so the old copy was moved instead: [`crate::data_move::Record::leaves`]). For Settings.
pub fn moved_leaving() -> Option<String> {
    static LEAVES: OnceLock<Option<String>> = OnceLock::new();
    LEAVES.get_or_init(|| crate::data_move::moved_in(&data_dir()).map(|m| m.leaves).filter(|l| !l.is_empty())).clone()
}

/// A folder the system names in `var`. An empty or relative one counts as none (as the XDG rules say), so the app
/// never keeps its files inside whatever folder it happened to be started from.
fn folder_var(var: &str) -> Option<PathBuf> {
    absolute(std::env::var_os(var).map(PathBuf::from))
}

fn absolute(path: Option<PathBuf>) -> Option<PathBuf> {
    path.filter(|p| p.is_absolute())
}

/// The diagnostics folder: session logs, crash logs and F12 notes' neighbours.
pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_logs_and_the_launchers_folders_are_no_data() {
        let dir = std::env::temp_dir().join(format!("mo-paths-holds-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("logs")).unwrap();
        fs::create_dir_all(dir.join("run-latest-rust")).unwrap();
        assert!(!holds_data(&dir));
        // A run opened with no data holds its lock file there, and the notes' (the whole-app review of 26 Nov, BD-3).
        for lock in crate::data_move::RUNNING_LOCKS {
            fs::write(dir.join(lock), "").unwrap();
        }
        assert!(!holds_data(&dir));
        assert!(!is_old_copy(&dir));
        fs::write(dir.join("rust-settings.json"), "{}").unwrap();
        assert!(holds_data(&dir));
        assert!(!is_old_copy(&dir), "data, but no move was made from it");
        fs::write(dir.join(crate::data_move::MOVED_OUT_FILE), "moved").unwrap();
        assert!(is_old_copy(&dir));
        let _ = fs::remove_dir_all(&dir);
        assert!(!holds_data(&dir), "no folder at all");
    }

    #[test]
    fn an_empty_or_relative_folder_counts_as_none() {
        assert_eq!(absolute(Some(PathBuf::new())), None);
        assert_eq!(absolute(Some(PathBuf::from("MusicData"))), None);
        let real = std::env::temp_dir();
        assert_eq!(absolute(Some(real.clone())), Some(real));
    }

    fn test_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("mo-paths-test").join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_pointer_is_followed_only_to_a_finished_move() {
        let root = test_folder("pointer");
        let first = root.join("MusicOrganiser");
        let moved = root.join("elsewhere").join("MusicOrganiser");
        fs::create_dir_all(&first).unwrap();
        // No pointer: the first folder, no problem.
        let r = resolve_in(&first);
        assert_eq!((r.folder.as_path(), r.problem), (first.as_path(), None));

        // A pointer to a folder that isn't there: the first folder, and a problem to show.
        fs::write(pointer_file(&first), format!("{}\r\n", moved.display())).unwrap();
        let r = resolve_in(&first);
        assert_eq!(r.folder, first);
        assert_eq!(r.missing.as_deref(), Some(moved.as_path()));
        assert!(r.problem.unwrap().contains("can't be found"));

        // There, but no move finished in it: still the first folder.
        fs::create_dir_all(&moved).unwrap();
        assert_eq!(resolve_in(&first).folder, first);

        // A finished move: its folder.
        fs::write(moved.join(MOVED_IN_FILE), "{}").unwrap();
        let r = resolve_in(&first);
        assert_eq!((r.folder.as_path(), r.problem), (moved.as_path(), None));

        // A folder the installer started the data in, with no move: its folder too.
        let started = root.join("picked").join("MusicOrganiser");
        fs::create_dir_all(&started).unwrap();
        fs::write(started.join(crate::install_choice::STARTED_HERE_FILE), "").unwrap();
        fs::write(pointer_file(&first), format!("{}\r\n", started.display())).unwrap();
        let r = resolve_in(&first);
        assert_eq!((r.folder.as_path(), r.problem), (started.as_path(), None));

        // A relative path is never followed.
        fs::write(pointer_file(&first), "MusicOrganiser").unwrap();
        let r = resolve_in(&first);
        assert_eq!(r.folder, first);
        assert!(r.problem.is_some());
        assert!(r.missing.is_none(), "a pointer no drive fixes isn't asked about at start");
        let _ = fs::remove_dir_all(&root);
    }
}
