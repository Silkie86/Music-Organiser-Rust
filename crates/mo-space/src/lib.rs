//! Space: how much room your songs, plugins and the app's own files take, and what could go to free some.
//!
//! - [`size`]: measuring files and folders without following links, and saying sizes ("38.2 GB").
//! - [`song`]: a song's room on disk and its Compact list (copies, old autosaves, caches, thinning, unused audio).
//! - [`clean`]: removing what you picked: a preview first, then only ever to the Recycle Bin, and undo.
//! - [`app_files`]: the app's own databases and folders, and whether their drive is nearly full.
//!
//! Nothing here touches the window or a database: the app reads what it needs from the library and hands it in, and
//! every walk is spread over the cores of the pool it's called in (call it in the app's low-priority pool).
//! Rules kept from the C# app: samples are never removed (rule 2), a song's newest save always stays, and nothing is
//! ever deleted for good.

pub mod app_files;
pub mod clean;
pub mod size;
pub mod song;

pub use size::{Measured, Stopped, size_text};

/// A path as Space compares paths: forward slashes, no slash at the end, and ignoring letter case (as Windows and the
/// library do).
pub fn key(path: &str) -> String {
    let key = mo_formats::case::key(&path.replace('\\', "/"));
    match key.trim_end_matches('/') {
        "" => "/".to_owned(),
        trimmed => trimmed.to_owned(),
    }
}
