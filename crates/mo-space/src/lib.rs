//! Space: how much room your songs, plugins and the app's own files take, and what could go to free some.
//!
//! - [`size`]: measuring files and folders without following links, and saying sizes ("38.2 GB").
//! - [`song`]: a song's room on disk and its Compact list (copies, old autosaves, caches, thinning, unused audio).
//! - [`clean`]: removing what you picked: a preview first, then only ever to the Recycle Bin, and undo.
//! - [`app_files`]: the app's own databases and folders, and whether their drive is nearly full.
//! - [`plugins`]: the plugin files installed on this computer.
//! - [`plugin_space`]: each installed plugin's room on disk, its data folders guessed by name and maker.
//! - [`plugin_clean`]: which of a plugin's preset and sound folders may go, and sending them to the Recycle Bin.
//! - [`spare`]: plugin copies in a format no project uses (or AAX without Pro Tools).
//! - [`uninstallers`]: a plugin's own uninstaller from the programs Windows lists, and running it.
//!
//! Nothing here touches the window or a database: the app reads what it needs from the library and hands it in, and
//! every walk is spread over the cores of the pool it's called in (call it in the app's low-priority pool).
//! Rules kept from the C# app: samples are never removed (rule 2), a song's newest save always stays, and nothing is
//! ever deleted for good.

pub mod app_files;
pub mod clean;
pub mod plugin_clean;
pub mod plugin_space;
pub mod plugins;
pub mod size;
pub mod song;
pub mod spare;
pub mod uninstallers;

pub use size::{Measured, Stopped, size_text};

/// A path as Space compares paths: forward slashes, no slash at the end, ignoring letter case (as Windows and the
/// library do), and without Windows' long-path prefix (`\\?\C:\…` is `C:\…`, `\\?\UNC\server\…` is `\\server\…`).
pub fn key(path: &str) -> String {
    let slashes = path.replace('\\', "/");
    let plain = match (slashes.strip_prefix("//?/UNC/"), slashes.strip_prefix("//?/")) {
        (Some(network), _) => format!("//{network}"),
        (None, Some(local)) => local.to_owned(),
        (None, None) => slashes,
    };
    let key = mo_formats::case::key(&plain);
    match key.trim_end_matches('/') {
        "" => "/".to_owned(),
        trimmed => trimmed.to_owned(),
    }
}

/// Your home folder (`HOME`, or `USERPROFILE` on Windows).
pub(crate) fn home_folder() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(std::path::PathBuf::from)
}

/// Where a file or folder really is, links and `..` worked out (a mapped network drive comes out as `\\server\…`), as
/// a plain path without Windows' long-path prefix; `None` when it isn't there.
pub fn real_path(path: &str) -> Option<String> {
    let real = std::fs::canonicalize(path).ok()?;
    let real = real.to_string_lossy();
    Some(match (real.strip_prefix(r"\\?\UNC\"), real.strip_prefix(r"\\?\")) {
        (Some(network), _) => format!(r"\\{network}"),
        (None, Some(local)) => local.to_owned(),
        (None, None) => real.into_owned(),
    })
}
