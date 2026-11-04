//! Space: how much room your songs, plugins and the app's own files take, and what could go to free some.
//!
//! - [`size`]: measuring files and folders without following links, and saying sizes ("38.2 GB").
//! - [`song`]: a song's room on disk and its Compact list (copies, old autosaves, caches, thinning, unused audio).
//! - [`clean`]: removing what you picked: a preview first, then only ever to the Recycle Bin, and undo.
//! - [`app_files`]: the app's own databases and folders, and whether their drive is nearly full.
//! - [`plugins`]: the plugin files installed on this computer.
//! - [`plugin_space`]: each installed plugin's room on disk, its data folders guessed by name and maker.
//! - [`plugin_clean`]: which of a plugin's preset and sound folders may go, and sending them to the Recycle Bin.
//! - [`spare`]: plugin copies in a format no project uses (or AAX without Pro Tools), and sending the ones you pick to
//!   the Recycle Bin.
//! - [`uninstallers`]: a plugin's own uninstaller from the programs Windows lists, and running it.
//! - [`folder_move`]: moving a plugin's sounds or presets to another drive with a link in their place, and back.
//! - [`half_copy`]: the marker in each copy a move makes until it's in use, so a copy a crash cut off is cleared.
//! - [`after_move`]: what can happen to a moved folder later (lost links, updates), and putting it right.
//! - [`old_copies`]: the old copies and other folders moves leave behind, with their size, to remind you of every time.
//! - [`in_bin`]: what the app has put in the Recycle Bin that's still there, to remind you: the app never empties it.
//! - [`admin`]: the administrator step, for folders, links and files Windows guards.
//! - [`bitwig`]: Bitwig's copies of Library multisamples inside projects, found byte for byte, and binned once a
//!   "(fixed)" project uses the Library file instead.
//!
//! Nothing here touches the window or a database: the app reads what it needs from the library and hands it in, and
//! every walk is spread over the cores of the pool it's called in (call it in the app's low-priority pool).
//! Rules kept from the C# app: samples are never removed (rule 2), a song's newest save always stays, and what goes
//! goes to the Recycle Bin, never for good. The one exception to rule 2, Solomon's choice (31 Oct 2026): a Bitwig
//! project's copy of a Library multisample goes to the Recycle Bin once a "(fixed)" project uses the Library file
//! instead ([`bitwig::remove_copies`]). Removed for good, as the C# app did too: only copies a folder move made itself,
//! when the move stops or fails part way, and (Solomon, 3 Nov 2026) half copies, a move's copies that a crash or a
//! power cut cut off ([`half_copy`]). They only ever hold copies, and the folder they copy from is left as it was.

pub mod admin;
pub mod after_move;
pub mod app_files;
pub mod bitwig;
pub mod clean;
pub mod folder_move;
pub mod half_copy;
pub mod in_bin;
pub mod old_copies;
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

/// A path as text (anything that isn't Unicode shown as �).
pub(crate) fn text(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A folder is there (a link to one counts).
pub(crate) fn is_dir(path: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_dir())
}

/// A real folder is there, not a link.
pub(crate) fn real_folder(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// Nothing is there at all, not even a link leading nowhere. A path that can't be looked at isn't counted as gone: it
/// may well still be there.
pub(crate) fn not_there(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
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

/// The folder a path is in.
pub(crate) fn parent_of(path: &str) -> Option<String> {
    std::path::Path::new(path.trim_end_matches(['/', '\\'])).parent().map(text)
}

/// What's in each of `places`, each place once (by where it really is); a place that can't be read is passed over.
/// Looks at `stop` before each one.
pub(crate) fn in_places(
    places: Vec<String>,
    stop: &std::sync::atomic::AtomicBool,
) -> Result<Vec<std::fs::DirEntry>, Stopped> {
    let mut looked: Vec<String> = Vec::new();
    let mut found = Vec::new();
    for place in places {
        if place.is_empty() {
            continue;
        }
        let where_really = real_path(&place).map_or_else(|| key(&place), |r| key(&r));
        if looked.contains(&where_really) {
            continue;
        }
        looked.push(where_really);
        let Ok(entries) = std::fs::read_dir(&place) else { continue };
        for entry in entries.flatten() {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(Stopped);
            }
            found.push(entry);
        }
    }
    Ok(found)
}

/// [`real_path`] for a path; the path itself when where it really is can't be told (it isn't there yet).
pub(crate) fn really(path: &std::path::Path) -> std::path::PathBuf {
    real_path(&path.to_string_lossy()).map_or_else(|| path.to_path_buf(), std::path::PathBuf::from)
}
