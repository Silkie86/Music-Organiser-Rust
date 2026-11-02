//! Where the app keeps its files. The same folder as the C# app (`%LOCALAPPDATA%\MusicOrganiser` on Windows), so both
//! apps share the library while the Rust one grows (spec section 6).

use std::path::PathBuf;

/// The name of the app's own folder in the system's temp folder ([`temp_dir`]).
pub const TEMP_FOLDER: &str = "MusicOrganiser-Rust";

/// The app's data folder. Falls back to a folder beside the current one if the system gives no home at all.
pub fn data_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("MusicOrganiser")
}

/// The diagnostics folder: session logs, crash logs and F12 notes' neighbours.
pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

/// The app's own folder in the system's temp folder, for files it makes for a moment (a MIDI clip dragged out to a
/// DAW). Rule 28: the start-up sweep ([`crate::tidy`]) clears what has been in it a week. A DAW takes a dragged MIDI
/// file's notes in, but keeps a link to a dragged audio file, so audio is dragged from where it is, never from here.
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(TEMP_FOLDER)
}
