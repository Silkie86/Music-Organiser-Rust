//! Show in folder and Copy path, shared by every page that lists files (songs, sounds, presets…), and opening a
//! folder or the Recycle Bin, each reporting on the activity cards.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use mo_app::activity::Feed;
use mo_app::background;
use mo_app::log;
use mo_app::shell;

thread_local! {
    /// The clipboard, kept open: on Linux what's copied only stays while it is.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

/// Shows the file or folder at `path` in its folder (`folder`), or the folder when it has gone. Off the window's
/// thread.
pub fn reveal(feed: &Feed, path: &str, folder: &str) {
    let (path, folder) = (path.to_owned(), folder.to_owned());
    let named = path.clone();
    launch(feed, "Show in folder", "Couldn't show it in its folder", named, move || {
        shell::reveal(&path, &folder, shell::exists(&path), shell::Os::THIS)
    });
}

/// Opens a folder (the app's own, say) in Explorer. Off the window's thread.
pub fn open_folder(feed: &Feed, folder: &str) {
    let folder = folder.to_owned();
    let named = folder.clone();
    launch(feed, "Open folder", "Couldn't open the folder", named, move || shell::open(&folder, None, shell::Os::THIS));
}

/// Opens the Recycle Bin, where everything the app removes goes first. Off the window's thread.
pub fn open_recycle_bin(feed: &Feed) {
    launch(feed, "Recycle Bin", "Couldn't open the Recycle Bin", "the Recycle Bin".to_owned(), || {
        shell::recycle_bin(&std::env::var("HOME").unwrap_or_default(), shell::Os::THIS)
    });
}

/// Runs what `make` makes (Explorer, Finder or the file manager) off the window's thread. If it can't, the log says
/// so under `job` with `what` it was for, and a card says `failed`.
fn launch(
    feed: &Feed,
    job: &'static str,
    failed: &'static str,
    what: String,
    make: impl FnOnce() -> shell::Launch + Send + 'static,
) {
    let feed = feed.clone();
    let started = background::spawn(job, move || {
        if let Err(e) = shell::run(&make()) {
            log::warning(job, &format!("{what}: {e}"));
            feed.start(failed, "").failed(&e.to_string());
        }
    });
    if let Err(e) = started {
        log::error(job, &format!("Couldn't start: {e}"));
    }
}

/// Puts a file's path on the clipboard.
pub fn copy_path(feed: &Feed, path: &str) {
    let copied = CLIPBOARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = Some(arboard::Clipboard::new()?);
        }
        match c.as_mut() {
            Some(clip) => clip.set_text(path.to_owned()),
            None => Ok(()),
        }
    });
    match copied {
        Ok(()) => feed.start("Copied the path", "").done(path),
        Err(e) => {
            log::warning("Copy path", &e.to_string());
            feed.start("Couldn't copy the path", "").failed(&e.to_string());
        }
    }
}

/// The Documents folder, where Windows really keeps it (it may have moved, to OneDrive say).
pub(crate) fn documents() -> PathBuf {
    #[cfg(windows)]
    if let Some(d) = known_folders::get_known_folder_path(known_folders::KnownFolder::Documents) {
        return d;
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    home.map_or_else(|| Path::new(".").to_owned(), |h| h.join("Documents"))
}
