//! What you give a song, kept in the app's own song library: its status, stars, heart, key, notes, tags and to-dos
//! ([`set`]).
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
//! A change to a song whose files are also found in other folders goes to those copies too, and the copy the song is
//! shown from is noted with it ([`set_all_keeping`], review SS-1), so the change can't make another copy take over.
//!
//! It waits for another write (up to five seconds), so call it away from the window's thread.

use std::path::Path;

use mo_formats::case;
use rusqlite::params;

use crate::song_writes::{begin_own, open_to_write, write_failed};
use crate::{LibraryError, STATUSES, Save, Todo};

/// The most stars a song can have.
pub const MOST_STARS: i32 = 5;
/// The longest tag kept, in characters.
pub const LONGEST_TAG: usize = 64;
/// The longest key kept, in characters ("F# minor", or however you write it).
pub const LONGEST_KEY: usize = 32;
/// The longest notes kept, in characters.
pub const LONGEST_NOTES: usize = 20_000;
/// The longest to-do kept, in characters: one line.
pub const LONGEST_TODO: usize = 200;
/// The most to-dos a song keeps, done ones too.
pub const MOST_TODOS: usize = 200;

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
    /// The song's whole to-do list as it is now (one added, ticked, or taken off): each one trimmed.
    Todos(Vec<Todo>),
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
            Change::Todos(todos) => {
                if todos.len() > MOST_TODOS {
                    return Err(format!("a song can have {MOST_TODOS} to-dos at most"));
                }
                todos
                    .iter()
                    .map(|t| {
                        let text = t.text.trim();
                        if text.is_empty() {
                            return Err("a to-do needs some words".to_owned());
                        }
                        shown("a to-do", text, LONGEST_TODO).map(|()| Todo { text: text.to_owned(), ..t.clone() })
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Change::Todos)
            }
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
            Change::Todos(todos) => {
                let differs = save.todos != todos;
                save.todos = todos;
                differs
            }
        }
    }
}

/// A tag as two spellings of it are compared, and as a new one is kept: without spaces around it, in small letters.
pub fn same_tag(tag: &str) -> String {
    case::to_lower(tag.trim())
}

/// Of a file found in several places, the one its song is shown from (`shown`) and the file's other places
/// (`copies`), as `song_choices` keeps it ([`crate::songs::SongChoices::is_kept`]): kept with a change given to the
/// song ([`set_all_keeping`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeepCopy {
    pub shown: String,
    pub copies: Vec<String>,
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
    set_all(songs_db, saves, std::slice::from_ref(change))
}

/// [`set`] for several changes, made in order in the one transaction: every one of them for every save, or nothing.
/// A tag put on with the tags the song shows, so its newest save holds them all (review SG-1).
pub fn set_all(songs_db: &Path, saves: &[SaveRow], changes: &[Change]) -> Result<Kept, LibraryError> {
    set_all_keeping(songs_db, saves, changes, &[])
}

/// [`set_all`], and in the same transaction the copies the song is shown from (`keep`, review SS-1): each `shown`
/// noted as a `kept` row of `song_choices`, its `copies` no longer, read back with the rest. A library without that
/// table keeps the changes alone (no table is made).
pub fn set_all_keeping(
    songs_db: &Path,
    saves: &[SaveRow],
    changes: &[Change],
    keep: &[KeepCopy],
) -> Result<Kept, LibraryError> {
    let write_error = |why: String| LibraryError::Write { path: songs_db.to_owned(), why };
    let sql = |e| write_failed(songs_db, e);
    let changes: Vec<Change> = changes.iter().map(Change::checked).collect::<Result<_, _>>().map_err(write_error)?;
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
    for change in &changes {
        make(&tx, &kept.saves, change).map_err(sql)?;
        // Read back: every save asked for has the change, before any of it is kept.
        for id in &kept.saves {
            if !made(&tx, *id, change).map_err(sql)? {
                return Err(write_error(format!("what was written didn't read back the same (save {id})")));
            }
        }
    }
    if !keep.is_empty() && has_choices(&tx).map_err(sql)? {
        for k in keep {
            keep_shown(&tx, k).map_err(sql)?;
        }
        for k in keep {
            if !shown_kept(&tx, k).map_err(sql)? {
                return Err(write_error(format!("what was written didn't read back the same ({})", k.shown)));
            }
        }
    }
    tx.commit().map_err(sql)?;
    Ok(kept)
}

/// A save noted as the copy its song is shown from ([`set_all_keeping`]) has moved from `from` to `to`: its note goes
/// with it, in `tx` (`song_writes::write`, with the save's row). Nothing when it had none, or for a library without
/// `song_choices`.
pub(crate) fn move_kept(tx: &rusqlite::Transaction<'_>, from: &str, to: &str) -> rusqlite::Result<()> {
    if has_choices(tx)? {
        tx.execute(
            "UPDATE OR REPLACE song_choices SET a = ?2 WHERE kind = 'kept' AND a = ?1",
            [case::to_lower(from), case::to_lower(to)],
        )?;
    }
    Ok(())
}

/// Whether the library has `song_choices` (every `songs.db` the app makes does).
fn has_choices(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<bool> {
    tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'song_choices')",
        [],
        |r| r.get(0),
    )
}

/// Notes `k.shown` as the copy its song is shown from, and none of `k.copies` (paths in small letters, as
/// `SongChoices` reads them), in `tx`.
fn keep_shown(tx: &rusqlite::Transaction<'_>, k: &KeepCopy) -> rusqlite::Result<()> {
    let mut forget = tx.prepare_cached("DELETE FROM song_choices WHERE kind = 'kept' AND a = ?1")?;
    for copy in &k.copies {
        forget.execute([case::to_lower(copy)])?;
    }
    tx.execute("INSERT OR REPLACE INTO song_choices (kind, a, b) VALUES ('kept', ?1, '')", [case::to_lower(&k.shown)])?;
    Ok(())
}

/// Whether `k` reads back as [`keep_shown`] made it, in `tx`.
fn shown_kept(tx: &rusqlite::Transaction<'_>, k: &KeepCopy) -> rusqlite::Result<bool> {
    let mut is_kept = tx.prepare_cached("SELECT EXISTS (SELECT 1 FROM song_choices WHERE kind = 'kept' AND a = ?1)")?;
    let shown: bool = is_kept.query_row([case::to_lower(&k.shown)], |r| r.get(0))?;
    let mut any_copy = false;
    for copy in &k.copies {
        any_copy |= is_kept.query_row([case::to_lower(copy)], |r| r.get::<_, bool>(0))?;
    }
    Ok(shown && !any_copy)
}

/// Makes `change` (checked) to the saves `ids`, in `tx`.
fn make(tx: &rusqlite::Transaction<'_>, ids: &[i64], change: &Change) -> rusqlite::Result<()> {
    let column = |column: &str, value: &dyn rusqlite::ToSql| -> rusqlite::Result<()> {
        let mut st = tx.prepare_cached(&format!("UPDATE projects SET {column} = ?2 WHERE id = ?1"))?;
        for id in ids {
            st.execute(params![id, value])?;
        }
        Ok(())
    };
    match change {
        Change::Status(status) => column("status", status),
        Change::Rating(stars) => column("rating", stars),
        Change::Favourite(heart) => column("favourite", heart),
        Change::Key(key) => column("musical_key", key),
        Change::Notes(notes) => column("notes", notes),
        Change::AddTag(tag) => {
            for id in ids {
                // One of another letter case (an older library's) is the same tag: it isn't put on twice.
                if tags_like(tx, *id, tag)?.is_empty() {
                    tx.execute("INSERT INTO tags (project_id, tag) VALUES (?1, ?2)", params![id, tag])?;
                }
            }
            Ok(())
        }
        Change::RemoveTag(tag) => {
            for id in ids {
                for spelt in tags_like(tx, *id, tag)? {
                    tx.execute("DELETE FROM tags WHERE project_id = ?1 AND tag = ?2", params![id, spelt])?;
                }
            }
            Ok(())
        }
        Change::Todos(todos) => {
            for id in ids {
                put_todos(tx, *id, todos)?;
            }
            Ok(())
        }
    }
}

/// Makes `todos` the save `id`'s whole to-do list, in their order, in `tx` (the table is there: [`begin_own`]).
pub(crate) fn put_todos(tx: &rusqlite::Transaction<'_>, id: i64, todos: &[Todo]) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM todos WHERE project_id = ?1", [id])?;
    let mut put =
        tx.prepare_cached("INSERT INTO todos (project_id, n, text, done, added) VALUES (?1, ?2, ?3, ?4, ?5)")?;
    for (n, t) in todos.iter().enumerate() {
        put.execute(params![id, n as i64, t.text, t.done, t.added])?;
    }
    Ok(())
}

/// The save `id`'s to-do list, in its order, in `tx`.
fn todos_of(tx: &rusqlite::Transaction<'_>, id: i64) -> rusqlite::Result<Vec<Todo>> {
    let mut st = tx.prepare_cached("SELECT text, done, added FROM todos WHERE project_id = ?1 ORDER BY n")?;
    st.query_map([id], |r| Ok(Todo { text: r.get(0)?, done: r.get(1)?, added: r.get(2)? }))?.collect()
}

/// Whether the save `id` reads back with `change` made ([`make`]), in `tx`.
fn made(tx: &rusqlite::Transaction<'_>, id: i64, change: &Change) -> rusqlite::Result<bool> {
    let text = |column: &str| -> rusqlite::Result<Option<String>> {
        tx.query_row(&format!("SELECT {column} FROM projects WHERE id = ?1"), [id], |r| r.get(0))
    };
    let number = |column: &str| -> rusqlite::Result<Option<i64>> {
        tx.query_row(&format!("SELECT {column} FROM projects WHERE id = ?1"), [id], |r| r.get(0))
    };
    Ok(match change {
        Change::Status(status) => text("status")?.as_deref() == Some(status.as_str()),
        Change::Rating(stars) => number("rating")? == Some(i64::from(*stars)),
        Change::Favourite(heart) => number("favourite")? == Some(i64::from(*heart)),
        Change::Key(key) => text("musical_key")?.as_deref() == Some(key.as_str()),
        Change::Notes(notes) => text("notes")?.as_deref() == Some(notes.as_str()),
        Change::AddTag(tag) => !tags_like(tx, id, tag)?.is_empty(),
        Change::RemoveTag(tag) => tags_like(tx, id, tag)?.is_empty(),
        Change::Todos(todos) => todos_of(tx, id)? == *todos,
    })
}

/// The tags the save `id` has that are `tag` ([`same_tag`]), as they're spelt. A save has a handful of tags, so
/// they're all looked at: letter case is compared as the app does, beyond English too.
pub(crate) fn tags_like(tx: &rusqlite::Transaction<'_>, id: i64, tag: &str) -> rusqlite::Result<Vec<String>> {
    let mut st = tx.prepare_cached("SELECT tag FROM tags WHERE project_id = ?1")?;
    let all: Vec<Option<String>> = st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(all.into_iter().flatten().filter(|t| same_tag(t) == tag).collect())
}
