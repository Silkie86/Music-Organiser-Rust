//! The data folder picked in the installer (Solomon, 2026-11-15: "option to put data folder where you want"). The
//! installer (`installer/setup.iss`) never copies or moves anyone's data itself. It leaves one of two small notes, and
//! the app does the rest:
//!
//! - **No data yet** (a new PC): the installer makes the picked folder, puts [`STARTED_HERE_FILE`] in it and points
//!   [`crate::paths::POINTER_FILE`] at it. The pointer follows a folder with that file as it follows a finished move,
//!   so the app starts out there and nothing is ever written to the first folder.
//! - **Data already there**: the installer leaves it where it is and, when another place was picked, writes
//!   [`MOVE_ASKED_FILE`] beside the first folder. The app then starts its own checked move to that place
//!   ([`crate::data_move`]: copy, check, switch at the next start), as Settings › Data's Move does, and takes the note
//!   away as it starts (rule 28), so the ask is only ever acted on once.

use std::fs;
use std::path::{Path, PathBuf};

/// In a data folder the installer started: the pointer may name it though no move went there.
pub const STARTED_HERE_FILE: &str = "started-here.txt";

/// Beside the first data folder (`%LOCALAPPDATA%\MusicOrganiser-move-asked.txt`): one line, the folder the data should
/// move to, written by the installer when data was already there.
pub const MOVE_ASKED_FILE: &str = "MusicOrganiser-move-asked.txt";

/// Where [`MOVE_ASKED_FILE`] sits for the first data folder `first`.
pub fn move_asked_file(first: &Path) -> PathBuf {
    first.with_file_name(MOVE_ASKED_FILE)
}

/// The folder the installer asked the data to move to, when it's a whole path and not the folder in use already. The
/// ask is dropped (its note taken away) when it can't be read, names no whole path, or names the folder in use.
pub fn move_asked_in(first: &Path, in_use: &Path) -> Option<PathBuf> {
    let file = move_asked_file(first);
    let text = fs::read_to_string(&file).ok()?;
    let to = PathBuf::from(text.trim());
    let same = mo_formats::case::key(to.to_string_lossy().trim_end_matches(['\\', '/']))
        == mo_formats::case::key(in_use.to_string_lossy().trim_end_matches(['\\', '/']));
    if to.is_absolute() && !same {
        Some(to)
    } else {
        let _ = fs::remove_file(&file);
        None
    }
}

/// [`move_asked_in`] for this run's folders.
pub fn move_asked() -> Option<PathBuf> {
    move_asked_in(&crate::paths::first_dir(), &crate::paths::data_dir())
}

/// Takes the installer's ask away, as the move it asked for starts.
pub fn take_move_ask() {
    let _ = fs::remove_file(move_asked_file(&crate::paths::first_dir()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("mo-install-choice-test").join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_ask_names_a_whole_other_folder_or_goes() {
        let root = test_folder("ask");
        let first = root.join("MusicOrganiser");
        fs::create_dir_all(&first).unwrap();
        let file = move_asked_file(&first);
        assert_eq!(move_asked_in(&first, &first), None, "no ask");

        let to = root.join("I").join("MusicOrganiser");
        fs::write(&file, format!("{}\r\n", to.display())).unwrap();
        assert_eq!(move_asked_in(&first, &first), Some(to.clone()));
        assert!(file.is_file(), "kept until the move starts");

        // The folder in use already (whatever the case or a last slash): nothing to do, and the ask goes.
        let upper = format!("{}{}", to.display().to_string().to_uppercase(), std::path::MAIN_SEPARATOR);
        fs::write(&file, &upper).unwrap();
        assert_eq!(move_asked_in(&first, &PathBuf::from(to.display().to_string().to_uppercase())), None);
        assert!(!file.exists());

        // Not a whole path: the ask goes.
        fs::write(&file, "MusicData").unwrap();
        assert_eq!(move_asked_in(&first, &first), None);
        assert!(!file.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
