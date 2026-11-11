//! What you give a song, kept in the app's own song library: its status, stars, heart and tags ([`set`]). Rule 14:
//! they're the song's, so one change goes to every save of the song the caller names, in one SQLite transaction
//! (IMMEDIATE, so nothing else writes meanwhile), read back and checked, all of it or none. Only a `songs.db` that's
//! the app's own is ever written ([`crate::song_writes`]).
//!
//! A save read again never sets these on a row the library has ([`crate::song_writes`]), so a change kept here stays
//! whatever the folder watch reads meanwhile. The caller makes the same change to the saves it holds in memory
//! ([`Change::apply`]) once it's kept, so what's shown is what's kept.
//!
//! It waits for another write (up to five seconds), so call it away from the window's thread.

use std::path::Path;

use mo_formats::case;
use rusqlite::params;

use crate::song_writes::{begin_own, open_to_write, write_failed};
use crate::{LibraryError, STATUSES, Save};

/// The most stars a song can have.
pub const MOST_STARS: i32 = 5;
/// The longest tag kept, in characters.
pub const LONGEST_TAG: usize = 64;

/// One change to what you gave a song.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// One of [`STATUSES`], as spelt there.
    Status(String),
    /// 0 (none) to [`MOST_STARS`].
    Rating(i32),
    Favourite(bool),
    /// Put a tag on: kept in small letters, as the app's tags are, without spaces around it.
    AddTag(String),
    /// Take a tag off, whatever its letter case.
    RemoveTag(String),
}

impl Change {
    /// The change as it's kept: a tag trimmed and in small letters. Why it can't be kept, otherwise: a status that
    /// isn't one of the app's, stars out of range, or a tag that's empty, too long or holds a character that can't be
    /// shown (the list read keeps a save's tags in one text, apart by such a character).
    pub fn checked(&self) -> Result<Change, String> {
        let tag = |tag: &str| -> Result<String, String> {
            let tag = case::to_lower(tag.trim());
            if tag.is_empty() {
                return Err("a tag needs a name".to_owned());
            }
            if tag.chars().count() > LONGEST_TAG {
                return Err(format!("a tag can be {LONGEST_TAG} characters at most"));
            }
            if tag.chars().any(char::is_control) {
                return Err("a tag can't hold a character that can't be shown".to_owned());
            }
            Ok(tag)
        };
        match self {
            Change::Status(status) => STATUSES
                .iter()
                .find(|s| **s == status)
                .map(|s| Change::Status((*s).to_owned()))
                .ok_or_else(|| format!("\"{status}\" isn't one of the app's statuses")),
            Change::Rating(stars) if (0..=MOST_STARS).contains(stars) => Ok(self.clone()),
            Change::Rating(stars) => Err(format!("{stars} stars isn't between 0 and {MOST_STARS}")),
            Change::Favourite(_) => Ok(self.clone()),
            Change::AddTag(t) => tag(t).map(Change::AddTag),
            Change::RemoveTag(t) => tag(t).map(Change::RemoveTag),
        }
    }

    /// Makes the change to a save held in memory, as [`set`] keeps it in the library: call it for each save [`set`]
    /// says it changed ([`Kept::saves`]), once that returned. Whether the save is any different for it.
    pub fn apply(&self, save: &mut Save) -> bool {
        let Ok(change) = self.checked() else { return false };
        match change {
            Change::Status(status) => {
                let differs = save.status != status;
                save.status = status;
                differs
            }
            Change::Rating(stars) => {
                let differs = save.rating != stars;
                save.rating = stars;
                differs
            }
            Change::Favourite(heart) => {
                let differs = save.favourite != heart;
                save.favourite = heart;
                differs
            }
            Change::AddTag(tag) => {
                let has = save.tags.iter().any(|t| case::to_lower(t) == tag);
                if !has {
                    save.tags.push(tag);
                }
                !has
            }
            Change::RemoveTag(tag) => {
                let before = save.tags.len();
                save.tags.retain(|t| case::to_lower(t) != tag);
                save.tags.len() != before
            }
        }
    }
}

/// What [`set`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    /// The saves the change is now kept for, by id, in the order asked: those of `save_ids` the library has.
    pub saves: Vec<i64>,
    /// Those of `save_ids` the library doesn't have (a save it wouldn't take, or one another copy of the app kept
    /// under another id): nothing is kept for them.
    pub unknown: Vec<i64>,
}

/// Keeps `change` for the saves `save_ids` (every save of one song, rule 14) in the app's own `songs.db` at
/// `songs_db`: all of them or none. An id asked twice counts once. A change that can't be kept ([`Change::checked`])
/// or a library that isn't the app's own changes nothing; a library busy with another write for longer than the wait
/// gives [`LibraryError::Busy`], and trying again in a moment can work.
pub fn set(songs_db: &Path, save_ids: &[i64], change: &Change) -> Result<Kept, LibraryError> {
    let write_error = |why: String| LibraryError::Write { path: songs_db.to_owned(), why };
    let sql = |e| write_failed(songs_db, e);
    let change = change.checked().map_err(write_error)?;
    let mut conn = open_to_write(songs_db)?;
    let tx = begin_own(&mut conn, songs_db)?;
    let mut kept = Kept::default();
    {
        let mut has = tx.prepare("SELECT EXISTS (SELECT 1 FROM projects WHERE id = ?1)").map_err(sql)?;
        for id in save_ids {
            if kept.saves.contains(id) || kept.unknown.contains(id) {
                continue;
            }
            let there: bool = has.query_row([id], |r| r.get(0)).map_err(sql)?;
            if there { kept.saves.push(*id) } else { kept.unknown.push(*id) }
        }
    }
    for id in &kept.saves {
        match &change {
            Change::Status(status) => {
                tx.execute("UPDATE projects SET status = ?2 WHERE id = ?1", params![id, status]).map_err(sql)?;
            }
            Change::Rating(stars) => {
                tx.execute("UPDATE projects SET rating = ?2 WHERE id = ?1", params![id, stars]).map_err(sql)?;
            }
            Change::Favourite(heart) => {
                tx.execute("UPDATE projects SET favourite = ?2 WHERE id = ?1", params![id, heart]).map_err(sql)?;
            }
            Change::AddTag(tag) => {
                // One of another letter case (an older library's) is the same tag: it isn't put on twice.
                if !tags_like(&tx, *id, tag).map_err(sql)?.is_empty() {
                    continue;
                }
                tx.execute("INSERT INTO tags (project_id, tag) VALUES (?1, ?2)", params![id, tag]).map_err(sql)?;
            }
            Change::RemoveTag(tag) => {
                for spelt in tags_like(&tx, *id, tag).map_err(sql)? {
                    tx.execute("DELETE FROM tags WHERE project_id = ?1 AND tag = ?2", params![id, spelt])
                        .map_err(sql)?;
                }
            }
        }
    }
    // Read back: every save asked for has the change, before any of it is kept.
    for id in &kept.saves {
        let right = match &change {
            Change::Status(status) => {
                let back: Option<String> =
                    tx.query_row("SELECT status FROM projects WHERE id = ?1", [id], |r| r.get(0)).map_err(sql)?;
                back.as_deref() == Some(status.as_str())
            }
            Change::Rating(stars) => {
                let back: Option<i32> =
                    tx.query_row("SELECT rating FROM projects WHERE id = ?1", [id], |r| r.get(0)).map_err(sql)?;
                back == Some(*stars)
            }
            Change::Favourite(heart) => {
                let back: Option<bool> =
                    tx.query_row("SELECT favourite FROM projects WHERE id = ?1", [id], |r| r.get(0)).map_err(sql)?;
                back == Some(*heart)
            }
            Change::AddTag(tag) => !tags_like(&tx, *id, tag).map_err(sql)?.is_empty(),
            Change::RemoveTag(tag) => tags_like(&tx, *id, tag).map_err(sql)?.is_empty(),
        };
        if !right {
            return Err(write_error(format!("what was written didn't read back the same (save {id})")));
        }
    }
    tx.commit().map_err(sql)?;
    Ok(kept)
}

/// The tags the save `id` has that are `tag` but for letter case, as they're spelt. A save has a handful of tags, so
/// they're all looked at: letter case is compared as the app does, beyond English too.
fn tags_like(tx: &rusqlite::Transaction<'_>, id: i64, tag: &str) -> rusqlite::Result<Vec<String>> {
    let mut st = tx.prepare_cached("SELECT tag FROM tags WHERE project_id = ?1")?;
    let all: Vec<Option<String>> = st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(all.into_iter().flatten().filter(|t| case::to_lower(t) == tag).collect())
}
