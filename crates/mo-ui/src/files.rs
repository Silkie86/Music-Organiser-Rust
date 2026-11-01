//! Show in folder and Copy path, shared by every page that lists files (songs, sounds, presets…), each reporting on
//! the activity cards.

use std::cell::RefCell;

use mo_app::activity::Feed;
use mo_app::background;
use mo_app::log;
use mo_app::shell;

thread_local! {
    /// The clipboard, kept open: on Linux what's copied only stays while it is.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

/// Shows the file at `path` in its folder (`folder`), or the folder when the file has gone. Off the window's thread.
pub fn reveal(feed: &Feed, path: &str, folder: &str) {
    let (feed, path, folder) = (feed.clone(), path.to_owned(), folder.to_owned());
    let started = background::spawn("Show in folder", move || {
        let launch = shell::reveal(&path, &folder, shell::exists(&path), shell::Os::THIS);
        if let Err(e) = shell::run(&launch) {
            log::warning("Show in folder", &format!("{path}: {e}"));
            feed.start("Couldn't show the file", "").failed(&e.to_string());
        }
    });
    if let Err(e) = started {
        log::error("Show in folder", &format!("Couldn't start: {e}"));
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
