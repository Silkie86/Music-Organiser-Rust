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
use std::time::{Duration, Instant};

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
    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => {
            let _ = fs::remove_file(&file);
            return None;
        }
    };
    // The installer writes UTF-8, which may start with a byte-order mark.
    let to = PathBuf::from(text.trim_start_matches('\u{feff}').trim());
    if to.is_absolute() && !crate::data_move::paths_match(&to, in_use) {
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

/// Takes the installer's ask away, once the copy it asked for has ended (done, stopped or failed): a copy cut short
/// by closing the app keeps it, so the next start goes on with it (DA-3).
pub fn take_move_ask() {
    let _ = fs::remove_file(move_asked_file(&crate::paths::first_dir()));
}

/// The uninstaller starts the app with this (installer/setup.iss, at the uninstall step, once it's been confirmed)
/// before it removes the files: the app and the background helper running from the same program are asked to close,
/// as Windows won't remove a program that runs (UP-12). The app first, so it can't start the helper again.
pub const CLOSE_FLAG: &str = "--close-for-setup";

/// What [`close_for_setup`] did: a line for each it asked, and whether each has ended.
pub struct Closed {
    pub lines: Vec<String>,
    pub all_ended: bool,
}

/// Asks the open app and the helper to close, when they run from `exe` (never a launcher's build or another
/// install), and waits up to `wait` for each process to end: not only its port, as the app lets go of its port
/// before its last moments.
pub fn close_for_setup(exe: &Path, wait: Duration) -> Closed {
    let exe = exe.display().to_string();
    let mut closed = Closed { lines: Vec::new(), all_ended: true };
    let ours = |other: &str| crate::one_copy::same_program(other, &exe) && !other.is_empty();
    let mut went = |what: &str, port: u16, pid: u32| {
        let ended = wait_for(port, pid, wait);
        closed.all_ended &= ended;
        closed.lines.push(format!("{what} (process {pid}): {}", if ended { "ended" } else { "still running" }));
    };
    if let Some(app) = crate::one_copy::hello(crate::one_copy::PORT).filter(|h| ours(&h.exe))
        && crate::one_copy::ask_to_close_for(crate::one_copy::PORT, "the uninstaller")
    {
        went("The app was asked to close", crate::one_copy::PORT, app.pid);
    }
    if let Some(helper) = crate::helper::hello(crate::helper::PORT).filter(|h| ours(&h.exe))
        && crate::helper::ask_to_quit(crate::helper::PORT)
    {
        went("The helper was asked to quit", crate::helper::PORT, helper.pid);
    }
    closed
}

/// Waits up to `wait` for `port` to be free and process `pid` to have ended: whether both happened.
fn wait_for(port: u16, pid: u32, wait: Duration) -> bool {
    let until = Instant::now() + wait;
    while crate::one_copy::taken(port) {
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    process_ended(pid, until.saturating_duration_since(Instant::now()))
}

/// Waits up to `wait` for process `pid` to end (Windows); elsewhere the free port is taken as the end.
#[cfg(windows)]
#[allow(unsafe_code)]
fn process_ended(pid: u32, wait: Duration) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: a handle opened here is waited on and closed here, and nothing else uses it.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        // It can't be opened: it has ended already.
        if handle.is_null() {
            return true;
        }
        let waited = WaitForSingleObject(handle, u32::try_from(wait.as_millis()).unwrap_or(u32::MAX));
        CloseHandle(handle);
        waited != WAIT_TIMEOUT
    }
}

#[cfg(not(windows))]
fn process_ended(_pid: u32, _wait: Duration) -> bool {
    true
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

        // As the installer writes it: UTF-8 with a byte-order mark.
        fs::write(&file, format!("\u{feff}{}\r\n", to.display())).unwrap();
        assert_eq!(move_asked_in(&first, &first), Some(to.clone()));

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
