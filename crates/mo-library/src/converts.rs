//! The Bitwig copies Open in Bitwig saves, remembered in the song library's settings as the C# app kept them
//! (`bitwig_convert:<file>`: when the copy was saved and which FL project it's from), so a later convert of the same
//! project may save over its own copy while nothing has changed it, instead of keeping it and adding "Name (2)" each
//! time (review X1-5). Only ever kept in the app's own `songs.db` ([`crate::songs_db::own_since`]); `library.db` is
//! never written here. As a record is kept, the records of copies that aren't there any more go, so they don't pile
//! up (rule 28). A record lost that way only means the next convert keeps the copy and takes the next name: never
//! that a copy is saved over.

use std::path::Path;

use crate::{Library, LibraryError, setting_list, songs_db};

/// What each record's setting is called, before the copy's path (the C# app's key).
pub const PREFIX: &str = "bitwig_convert:";

/// The setting that remembers the copy at `file`.
pub fn key(file: &Path) -> String {
    format!("{PREFIX}{}", file.display())
}

/// Remembers `written` (`mo_convert::save::written`) for the copy at `file` in the song library at `songs`, and lets go
/// of the records of copies that have gone. Only while `songs` is the app's own library: otherwise nothing is written
/// and it says so with `false`. Looks at the disk: never on the window's thread.
pub fn remember(songs: &Path, file: &Path, written: &str) -> Result<bool, LibraryError> {
    if songs_db::own_since(songs).is_none() {
        return Ok(false);
    }
    // Read and looked for before the write, so the library isn't held while the disk is looked at. A copy on a drive
    // that isn't plugged in is taken as gone: its record goes, and its next convert keeps it.
    let key = key(file);
    let gone: Vec<(String, String)> = Library::open_read_only(songs)?
        .settings_starting(PREFIX)?
        .into_iter()
        .filter(|(k, _)| *k != key && !Path::new(&k[PREFIX.len()..]).exists())
        .collect();
    setting_list::change_text(songs, &key, |now| (now != Some(written)).then(|| written.to_owned()))?;
    setting_list::forget(songs, &gone)?;
    Ok(true)
}
