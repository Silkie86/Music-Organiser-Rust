//! The Songs page's Board (1.1 plan step 4, mock-up agreed with Solomon on 27 Nov 2026): one column a status, Idea
//! to Released, with Archived folded away on the right; each song a card. The Idea column shows the ideas saved in
//! the last year, with how many older ones there are behind "Show all".
//!
//! Each card can carry a guess at how far along its song looks, from the shape of its newest save
//! ([`mo_formats::shape`], [`mo_library::stage`]): only when that's a later stage than yours, and not once you've said
//! "not for this save". Shapes are read once a save, in the background, and kept in songs.db.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::NaiveDateTime;
use mo_formats::shape::Shape;
use mo_library::song_writes::ShapeRead;
use mo_library::songs::Song;
use mo_library::{STATUSES, Save};
use rayon::prelude::*;

use crate::songs::{Entry, Songs, View};

/// An idea saved longer ago than this (days) is behind the Idea column's "Show all".
pub const IDEA_DAYS: i64 = 365;

/// How many saves are read before what was found is handed on (kept, and shown): small enough that the board fills
/// in as it goes and little is held at once.
pub const READ_BATCH: usize = 32;

/// A save's shape as it was read, with the file it's of (its time and size): it only counts while the save still has
/// them. None is a save that has no shape (a Studio One song, or one too damaged).
#[derive(Clone, Debug, PartialEq)]
pub struct KeptShape {
    pub mtime: i64,
    pub size: i64,
    pub shape: Option<Shape>,
}

/// A save whose shape is still to be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToRead {
    pub id: i64,
    pub path: String,
    pub mtime: i64,
    pub size: i64,
}

/// The save a song's guess is made from: its newest by its file's time (the stage guess goes by the newest save
/// only: Solomon's answer to F6, 24 Nov 2026), of those whose file is still there.
pub fn newest(song: &Song) -> &Save {
    song.saves().filter(|s| !s.file_missing).max_by_key(|s| s.mtime).unwrap_or(&song.latest)
}

/// Whether a save of this DAW has a shape to read (FL Studio and Bitwig; a Studio One song hasn't yet).
pub fn has_shape(daw: &str) -> bool {
    daw == "FL Studio" || daw == "Bitwig Studio"
}

/// Reads these saves' shapes, a few at a time on the bulk pool (they mostly wait on a disk), and hands each
/// [`READ_BATCH`] on to `found` as it's done. Stops between batches once `stop` is set (the app closing). A file that
/// can't be read is left out, to be tried again at the next start; one whose reader trips over it is kept as having
/// no shape, so it isn't tried again until it changes.
pub fn read_shapes(list: Vec<ToRead>, stop: &Arc<AtomicBool>, mut found: impl FnMut(Vec<ShapeRead>)) {
    for batch in list.chunks(READ_BATCH) {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let read: Vec<ShapeRead> = crate::background::on_bulk_pool(|| batch.par_iter().filter_map(read_one).collect());
        if !read.is_empty() {
            found(read);
        }
    }
}

fn read_one(save: &ToRead) -> Option<ShapeRead> {
    let path = std::path::Path::new(&save.path);
    let raw = std::fs::read(path).ok()?;
    // Read only as the file the library knows: one written since is read for its shape once the library has it.
    if i64::try_from(raw.len()).ok()? != save.size {
        return None;
    }
    let shape =
        mo_catch::catch(std::panic::AssertUnwindSafe(|| mo_formats::shape::read_bytes(path, raw))).unwrap_or_default();
    Some(ShapeRead { id: save.id, mtime: save.mtime, size: save.size, shape })
}

/// One column of the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    /// One of [`STATUSES`].
    pub status: &'static str,
    /// Its songs in the list's order, by their place in [`Songs::rows`].
    pub cards: Vec<usize>,
    /// How many of its songs aren't shown: older ideas, behind "Show all".
    pub older: usize,
}

/// The board's columns, one a status in [`STATUSES`]' order, from the songs the list shows (so the search box and
/// filters work on it as they do on the list). A song with a status the app doesn't know is an idea, as the library
/// reads it. `all_ideas` shows the ideas saved more than [`IDEA_DAYS`] ago (or never dated) too.
pub fn columns(view: &View, songs: &Songs, all_ideas: bool, now: NaiveDateTime) -> Vec<Column> {
    let mut columns: Vec<Column> =
        STATUSES.iter().map(|status| Column { status, cards: Vec::new(), older: 0 }).collect();
    let since = now - chrono::Duration::days(IDEA_DAYS);
    for entry in &view.entries {
        let Entry::Song(row) = entry else { continue };
        let Some(r) = songs.rows.get(*row) else { continue };
        let at = STATUSES.iter().position(|s| s.eq_ignore_ascii_case(&r.status)).unwrap_or(0);
        let Some(column) = columns.get_mut(at) else { continue };
        if at == 0 && !all_ideas && r.saved.is_none_or(|saved| saved < since) {
            column.older += 1;
        } else {
            column.cards.push(*row);
        }
    }
    columns
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn only_fl_and_bitwig_saves_have_a_shape() {
        assert!(has_shape("FL Studio"));
        assert!(has_shape("Bitwig Studio"));
        assert!(!has_shape("Studio One"));
    }

    #[test]
    fn a_file_that_cant_be_read_or_has_changed_is_left_for_later() {
        let dir = std::env::temp_dir().join(format!("mo-board-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s1 = dir.join("Night.song");
        std::fs::write(&s1, b"not a song").unwrap();
        let gone = dir.join("Gone.flp").to_string_lossy().into_owned();
        let list = vec![
            ToRead { id: 1, path: s1.to_string_lossy().into_owned(), mtime: 5, size: 10 },
            ToRead { id: 2, path: gone, mtime: 5, size: 10 },
            ToRead { id: 3, path: s1.to_string_lossy().into_owned(), mtime: 5, size: 11 },
        ];
        let mut found = Vec::new();
        read_shapes(list, &Arc::new(AtomicBool::new(false)), |batch| found.extend(batch));
        // Read, and found to have no shape (Studio One); the missing one and the changed one wait.
        assert_eq!(found, vec![ShapeRead { id: 1, mtime: 5, size: 10, shape: None }]);
        let mut none = Vec::new();
        let list = vec![ToRead { id: 1, path: s1.to_string_lossy().into_owned(), mtime: 5, size: 10 }];
        read_shapes(list, &Arc::new(AtomicBool::new(true)), |batch| none.extend(batch));
        assert!(none.is_empty());
    }

    #[test]
    fn each_song_goes_in_its_status_column_and_old_ideas_wait_behind_show_all() {
        let now = chrono::Local::now().naive_local();
        let save = |id: i64, name: &str, status: &str, days: i64| Save {
            id,
            name: name.into(),
            daw: "FL Studio".into(),
            folder: format!(r"C:\Music\{name}"),
            path: format!(r"C:\Music\{name}\{name}.flp"),
            modified: now - chrono::Duration::days(days),
            mtime: (now - chrono::Duration::days(days)).and_utc().timestamp(),
            size: 1,
            status: status.into(),
            ..Save::default()
        };
        let saves = vec![
            save(1, "Kite", "Idea", 3),
            save(2, "Tape", "Idea", 900),
            save(3, "Night", "Mixing", 5),
            save(4, "Dust", "Archived", 50),
            save(5, "Odd", "Something else", 8),
        ];
        let songs = Songs::from_saves(&saves, &mo_library::SongChoices::default(), Vec::new(), 3, None);
        let view = songs.view(&crate::songs::Filters::default(), now);
        let names = |c: &Column| c.cards.iter().map(|&i| songs.rows[i].song.name.clone()).collect::<Vec<_>>();
        let columns = columns(&view, &songs, false, now);
        assert_eq!(columns.iter().map(|c| c.status).collect::<Vec<_>>(), STATUSES);
        assert_eq!(names(&columns[0]), ["Kite", "Odd"]);
        assert_eq!(columns[0].older, 1);
        assert_eq!(names(&columns[3]), ["Night"]);
        assert_eq!(names(&columns[6]), ["Dust"]);
        let all = super::columns(&view, &songs, true, now);
        assert_eq!(names(&all[0]), ["Kite", "Odd", "Tape"]);
        assert_eq!(all[0].older, 0);
    }
}
