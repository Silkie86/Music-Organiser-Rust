//! Where the app keeps its files. The same folder as the C# app (`%LOCALAPPDATA%\MusicOrganiser` on Windows), so both
//! apps share the library while the Rust one grows (spec section 6).

use std::path::PathBuf;

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
