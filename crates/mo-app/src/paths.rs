//! Where the app keeps its files. The same folder as the C# app (`%LOCALAPPDATA%\MusicOrganiser` on Windows), so both
//! apps share the library while the Rust one grows (spec section 6).

use std::path::PathBuf;

/// The app's data folder. Falls back to a folder beside the current one if the system gives no home at all.
pub fn data_dir() -> PathBuf {
    let base = if cfg!(windows) {
        folder_var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        folder_var("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        folder_var("XDG_DATA_HOME").or_else(|| folder_var("HOME").map(|h| h.join(".local").join("share")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("MusicOrganiser")
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
}
