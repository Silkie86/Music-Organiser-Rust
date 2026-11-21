//! The Bitwig copies Open in Bitwig saves, remembered in the song library's settings as the C# app kept them
//! (`bitwig_convert:<file>`: when the copy was saved and which FL project it's from), so a later convert of the same
//! project may save over its own copy while nothing has changed it, instead of keeping it and adding "Name (2)" each
//! time (review X1-5). Only ever kept in the app's own `songs.db` ([`crate::songs_db::own_since`]); `library.db` is
//! never written here. As a record is kept, the records of copies that aren't there any more go with it, in the same
//! transaction, so they don't pile up (rule 28). A record lost that way only means the next convert keeps the copy and
//! takes the next name: never that a copy is saved over.
//!
//! A second kind of record (`bitwig_opened:<file>`: when Bitwig was asked to open the copy) lasts a day, so a convert
//! Bitwig may still show keeps its name through a restart of the app (review CN-11, [`remember_opened`]). It isn't
//! the first kind's time over again: that one is the file's own save time and is kept for good, and a copy that
//! couldn't be opened has none of this kind. Losing one of these is the less safe way round: the next convert may
//! then replace a copy Bitwig still shows, as every convert could before there were any.

use std::path::{Path, PathBuf};

use crate::{Library, LibraryError, setting_list, songs_db};

/// What each record's setting is called, before the copy's path (the C# app's key).
pub const PREFIX: &str = "bitwig_convert:";

/// The setting that remembers the copy at `file`.
pub fn key(file: &Path) -> String {
    format!("{PREFIX}{}", file.display())
}

/// Remembers `written` (`mo_convert::save::written`) for the copy at `file` in the song library at `songs`, and lets go
/// of the records of copies that have gone, in one transaction. Only while `songs` is the app's own library: otherwise
/// nothing is written and it says so with `false`. Looks at the disk: never on the window's thread.
pub fn remember(songs: &Path, file: &Path, written: &str) -> Result<bool, LibraryError> {
    if songs_db::own_since(songs).is_none() {
        return Ok(false);
    }
    // Read and looked for before the write, so the library isn't held while the disk is looked at.
    let key = key(file);
    let gone: Vec<(String, String)> = Library::open_read_only(songs)?
        .settings_starting(PREFIX)?
        .into_iter()
        .filter(|(k, _)| *k != key && k.get(PREFIX.len()..).is_some_and(|copy| is_gone(Path::new(copy))))
        .collect();
    let edit = |now: Option<&str>| (now != Some(written)).then(|| written.to_owned());
    setting_list::change_text_and_forget(songs, &key, edit, &gone)?;
    Ok(true)
}

/// Whether the copy at `file` isn't there, as the disk says: on a drive that isn't plugged in, it isn't (its record
/// goes, and its next convert keeps it and takes the next name). One that can't be looked at just now (not allowed, or
/// a network drive that doesn't answer) keeps its record.
fn is_gone(file: &Path) -> bool {
    matches!(std::fs::symlink_metadata(file), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// What the setting that remembers a copy was opened in Bitwig is called, before the copy's path.
pub const OPENED_PREFIX: &str = "bitwig_opened:";

/// How long after a copy was opened in Bitwig it counts as maybe still open there (whole-app review CN-11, 19 Nov
/// 2026). Bitwig only brings an open project forward, and the app can't see whether it still has one open, so for a
/// day a new convert of the same song takes the next name, as it does while the app that opened it still runs.
pub const OPEN_FOR_SECONDS: u64 = 24 * 60 * 60;

/// The setting that remembers the copy at `file` was opened in Bitwig.
pub fn opened_key(file: &Path) -> String {
    format!("{OPENED_PREFIX}{}", file.display())
}

/// Whether a copy whose opened record is `opened` (seconds since 1970, as [`remember_opened`] keeps it) may still be
/// open in Bitwig at `now`. A record from a little after `now` (the clock was set back) counts as open, but one from
/// more than [`OPEN_FOR_SECONDS`] ahead says nothing, so a wrong clock never holds a name for good; one that can't be
/// read doesn't count either.
pub fn may_be_open(opened: Option<&str>, now: u64) -> bool {
    opened
        .and_then(|o| o.parse::<u64>().ok())
        .is_some_and(|at| now < at.saturating_add(OPEN_FOR_SECONDS) && at <= now.saturating_add(OPEN_FOR_SECONDS))
}

/// The copies opened in Bitwig that may still be open there at `now`, as `lib` remembers them.
pub fn opened_lately(lib: &Library, now: u64) -> Result<Vec<PathBuf>, LibraryError> {
    Ok(lib
        .settings_starting(OPENED_PREFIX)?
        .into_iter()
        .filter(|(_, at)| may_be_open(Some(at), now))
        .filter_map(|(k, _)| k.get(OPENED_PREFIX.len()..).map(PathBuf::from))
        .collect())
}

/// Remembers that the copy at `file` was opened in Bitwig at `now` (seconds since 1970) in the song library at
/// `songs`, and lets go of the records that say nothing any more (older than [`OPEN_FOR_SECONDS`], or of a copy that
/// has gone), in one transaction (rule 28). Only while `songs` is the app's own library, as [`remember`]. A record
/// lost only means a convert may replace a copy Bitwig still shows, as before there were records; none is ever a
/// reason to save over anything. Looks at the disk: never on the window's thread.
pub fn remember_opened(songs: &Path, file: &Path, now: u64) -> Result<bool, LibraryError> {
    if songs_db::own_since(songs).is_none() {
        return Ok(false);
    }
    let key = opened_key(file);
    let stale: Vec<(String, String)> = Library::open_read_only(songs)?
        .settings_starting(OPENED_PREFIX)?
        .into_iter()
        .filter(|(k, at)| {
            *k != key
                && (!may_be_open(Some(at), now)
                    || k.get(OPENED_PREFIX.len()..).is_some_and(|copy| is_gone(Path::new(copy))))
        })
        .collect();
    let at = now.to_string();
    let edit = |was: Option<&str>| (was != Some(at.as_str())).then(|| at.clone());
    setting_list::change_text_and_forget(songs, &key, edit, &stale)?;
    Ok(true)
}
