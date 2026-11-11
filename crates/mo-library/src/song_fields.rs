//! What you give a song, kept in the app's own song library: its status, stars, heart, key, notes and tags ([`set`]).
//! Rule 14: they're the song's, so one change goes to every save of the song the caller names, in one SQLite
//! transaction (IMMEDIATE, so nothing else writes meanwhile), read back and checked, all of it or none. Only a
//! `songs.db` that's the app's own is ever written ([`crate::song_writes`]).
//!
//! A save read again never sets these on a row the library has ([`crate::song_writes`]), so a change kept here stays
//! whatever the folder watch reads meanwhile. The caller makes the same change to the saves it holds in memory
//! ([`Change::apply`]), those the library doesn't have yet too: they're kept with it when the library takes them. The
//! Songs list does both in one step, on the thread that reads saves, so neither can come between
//! (`mo_app::live::Live::give`).
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
/// The longest key kept, in characters ("F# minor", or however you write it).
pub const LONGEST_KEY: usize = 32;
/// The longest notes kept, in characters.
pub const LONGEST_NOTES: usize = 20_000;

/// A save as the library has it: its id, and the path the library keeps it under. Only the row with both is changed,
/// as everywhere a save's row is written: an id alone may be another save's by now (another copy of the app kept one
/// under it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveRow {
    pub id: i64,
    pub path: String,
}

/// One change to what you gave a song.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// One of [`STATUSES`], as spelt there.
    Status(String),
    /// 0 (none) to [`MOST_STARS`].
    Rating(i32),
    Favourite(bool),
    /// The key you give it, as you write it (empty: none, so the key heard from its notes shows).
    Key(String),
    /// Your notes on it, as you write them.
    Notes(String),
    /// Put a tag on: kept in small letters, as the app's tags are, without spaces around it.
    AddTag(String),
    /// Take a tag off, whatever its letter case.
    RemoveTag(String),
}

impl Change {
    /// The change as it's kept: a tag or a key trimmed, a tag in small letters. Why it can't be kept, otherwise: a
    /// status that isn't one of the app's, stars out of range, a tag that's empty, a tag, key or notes too long, or a
    /// tag or key with a character that can't be shown (the list read keeps a save's tags in one text, apart by such
    /// a character). A tag to take off only needs a name: one an older library holds, however long, can go.
    pub fn checked(&self) -> Result<Change, String> {
        let shown = |what: &str, text: &str, longest: usize| -> Result<(), String> {
            if text.chars().count() > longest {
                return Err(format!("{what} can be {longest} characters at most"));
            }
            if text.chars().any(char::is_control) {
                return Err(format!("{what} can't hold a character that can't be shown"));
            }
            Ok(())
        };
        let named = |tag: &str| -> Result<String, String> {
            let tag = same_tag(tag);
            if tag.is_empty() { Err("a tag needs a name".to_owned()) } else { Ok(tag) }
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
            Change::Key(key) => {
                let key = key.trim();
                shown("a key", key, LONGEST_KEY).map(|()| Change::Key(key.to_owned()))
            }
            Change::Notes(notes) if notes.chars().count() > LONGEST_NOTES => {
                Err(format!("notes can be {LONGEST_NOTES} characters at most"))
            }
            Change::Notes(_) => Ok(self.clone()),
            Change::AddTag(tag) => {
                let tag = named(tag)?;
                shown("a tag", &tag, LONGEST_TAG).map(|()| Change::AddTag(tag))
            }
            Change::RemoveTag(tag) => named(tag).map(Change::RemoveTag),
        }
    }

    /// Makes the change to a save held in memory, as [`set`] keeps it in the library: for every save of the song,
    /// once [`set`] returned, those it says the library doesn't have too (they're kept as they are in memory when
    /// the library takes them). Whether the save is any different for it.
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
            Change::Key(key) => {
                let differs = save.musical_key != key;
                save.musical_key = key;
                differs
            }
            Change::Notes(notes) => {
                let differs = save.notes != notes;
                save.notes = notes;
                differs
            }
            Change::AddTag(tag) => {
                let has = save.tags.iter().any(|t| same_tag(t) == tag);
                if !has {
                    save.tags.push(tag);
                }
                !has
            }
            Change::RemoveTag(tag) => {
                let before = save.tags.len();
                save.tags.retain(|t| same_tag(t) != tag);
                save.tags.len() != before
            }
        }
    }
}

/// A tag as two spellings of it are compared, and as a new one is kept: without spaces around it, in small letters.
pub fn same_tag(tag: &str) -> String {
    case::to_lower(tag.trim())
}

/// What [`set`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    /// The saves the change is now kept for, by id, in the order asked.
    pub saves: Vec<i64>,
    /// Those asked for that the library doesn't have under that id and path (a save it hasn't taken yet, or one
    /// another copy of the app kept under another id): nothing is kept for them here.
    pub unknown: Vec<i64>,
}

/// Keeps `change` for `saves` (every save of one song, rule 14) in the app's own `songs.db` at `songs_db`: all of
/// them or none. A save asked twice counts once. A change that can't be kept ([`Change::checked`]) or a library that
/// isn't the app's own changes nothing; a library busy with another write for longer than the wait gives
/// [`LibraryError::Busy`], and trying again in a moment can work.
pub fn set(songs_db: &Path, saves: &[SaveRow], change: &Change) -> Result<Kept, LibraryError> {
    let write_error = |why: String| LibraryError::Write { path: songs_db.to_owned(), why };
    let sql = |e| write_failed(songs_db, e);
    let change = change.checked().map_err(write_error)?;
    let mut conn = open_to_write(songs_db)?;
    let tx = begin_own(&mut conn, songs_db)?;
    let mut kept = Kept::default();
    {
        let mut has = tx.prepare("SELECT EXISTS (SELECT 1 FROM projects WHERE id = ?1 AND path = ?2)").map_err(sql)?;
        for save in saves {
            if kept.saves.contains(&save.id) || kept.unknown.contains(&save.id) {
                continue;
            }
            let there: bool = has.query_row(params![save.id, save.path], |r| r.get(0)).map_err(sql)?;
            if there { kept.saves.push(save.id) } else { kept.unknown.push(save.id) }
        }
    }
    let column = |column: &str, value: &dyn rusqlite::ToSql| -> rusqlite::Result<()> {
        let mut st = tx.prepare_cached(&format!("UPDATE projects SET {column} = ?2 WHERE id = ?1"))?;
        for id in &kept.saves {
            st.execute(params![id, value])?;
        }
        Ok(())
    };
    match &change {
        Change::Status(status) => column("status", status).map_err(sql)?,
        Change::Rating(stars) => column("rating", stars).map_err(sql)?,
        Change::Favourite(heart) => column("favourite", heart).map_err(sql)?,
        Change::Key(key) => column("musical_key", key).map_err(sql)?,
        Change::Notes(notes) => column("notes", notes).map_err(sql)?,
        Change::AddTag(tag) => {
            for id in &kept.saves {
                // One of another letter case (an older library's) is the same tag: it isn't put on twice.
                if tags_like(&tx, *id, tag).map_err(sql)?.is_empty() {
                    tx.execute("INSERT INTO tags (project_id, tag) VALUES (?1, ?2)", params![id, tag]).map_err(sql)?;
                }
            }
        }
        Change::RemoveTag(tag) => {
            for id in &kept.saves {
                for spelt in tags_like(&tx, *id, tag).map_err(sql)? {
                    tx.execute("DELETE FROM tags WHERE project_id = ?1 AND tag = ?2", params![id, spelt])
                        .map_err(sql)?;
                }
            }
        }
    }
    // Read back: every save asked for has the change, before any of it is kept.
    let text = |column: &str, id: i64| -> rusqlite::Result<Option<String>> {
        tx.query_row(&format!("SELECT {column} FROM projects WHERE id = ?1"), [id], |r| r.get(0))
    };
    let number = |column: &str, id: i64| -> rusqlite::Result<Option<i64>> {
        tx.query_row(&format!("SELECT {column} FROM projects WHERE id = ?1"), [id], |r| r.get(0))
    };
    for id in &kept.saves {
        let right = match &change {
            Change::Status(status) => text("status", *id).map_err(sql)?.as_deref() == Some(status.as_str()),
            Change::Rating(stars) => number("rating", *id).map_err(sql)? == Some(i64::from(*stars)),
            Change::Favourite(heart) => number("favourite", *id).map_err(sql)? == Some(i64::from(*heart)),
            Change::Key(key) => text("musical_key", *id).map_err(sql)?.as_deref() == Some(key.as_str()),
            Change::Notes(notes) => text("notes", *id).map_err(sql)?.as_deref() == Some(notes.as_str()),
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

/// The tags the save `id` has that are `tag` ([`same_tag`]), as they're spelt. A save has a handful of tags, so
/// they're all looked at: letter case is compared as the app does, beyond English too.
pub(crate) fn tags_like(tx: &rusqlite::Transaction<'_>, id: i64, tag: &str) -> rusqlite::Result<Vec<String>> {
    let mut st = tx.prepare_cached("SELECT tag FROM tags WHERE project_id = ?1")?;
    let all: Vec<Option<String>> = st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(all.into_iter().flatten().filter(|t| same_tag(t) == tag).collect())
}
