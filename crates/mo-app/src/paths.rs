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
/// without it isn't followed (that folder isn't a finished move: a drive letter given to another disk, a folder made
/// by hand).
pub const MOVED_IN_FILE: &str = "moved-in.json";

/// The folder the data is in and, when a move's pointer couldn't be followed, why (for the log and Settings).
pub(crate) struct Resolved {
    pub(crate) folder: PathBuf,
    pub(crate) problem: Option<String>,
    /// The problem is that the folder a finished move went to isn't there now (its drive is missing, say), rather
    /// than a pointer that can't be read.
    pub(crate) missing: bool,
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
    now.problem.filter(|_| now.missing)
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
    let fallback = |problem: Option<String>| Resolved { folder: first.to_path_buf(), problem, missing: false };
    let text = match fs::read_to_string(pointer_file(first)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return fallback(None),
        Err(e) => return fallback(Some(format!("Couldn't read {POINTER_FILE}: {e}. Using {}", first.display()))),
    };
    let Some(named) = absolute(Some(PathBuf::from(text.trim()))) else {
        return fallback(Some(format!("{POINTER_FILE} doesn't name a whole folder path. Using {}", first.display())));
    };
    if named.join(MOVED_IN_FILE).is_file() {
        Resolved { folder: named, problem: None, missing: false }
    } else {
        Resolved {
            missing: true,
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
        assert!(r.missing);
        assert!(r.problem.unwrap().contains("can't be found"));

        // There, but no move finished in it: still the first folder.
        fs::create_dir_all(&moved).unwrap();
        assert_eq!(resolve_in(&first).folder, first);

        // A finished move: its folder.
        fs::write(moved.join(MOVED_IN_FILE), "{}").unwrap();
        let r = resolve_in(&first);
        assert_eq!((r.folder.as_path(), r.problem), (moved.as_path(), None));

        // A relative path is never followed.
        fs::write(pointer_file(&first), "MusicOrganiser").unwrap();
        let r = resolve_in(&first);
        assert_eq!(r.folder, first);
        assert!(r.problem.is_some());
        assert!(!r.missing, "a pointer no drive fixes isn't asked about at start");
        let _ = fs::remove_dir_all(&root);
    }
}
