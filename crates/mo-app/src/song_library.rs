//! Making the app's own song library, `songs.db`, from the C# app's `library.db` (step 2 of the new databases): once,
//! in the background, a little after the app starts, with a card that says how it went. The app switches to it at
//! the next start ([`mo_library::songs_db::at_start`]); until then, and if it can't be made, `library.db` is read as
//! before.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mo_library::songs::thousands;
use mo_library::songs_db::{self, Made};

use crate::activity::Feed;
use crate::log;

/// How long after the start it waits, so the pages read what they show first: making it reads the whole of
/// `library.db` (a minute or more on a slow drive).
const WAIT: Duration = Duration::from_secs(45);

/// Room left free on the drive beyond the new file.
const SPARE_ROOM: u64 = 1 << 30;

/// Makes `songs.db` in the data folder `dir` from the `library.db` there, on a low-priority thread of its own, after
/// [`WAIT`].
pub fn make_later(dir: PathBuf, feed: Feed) {
    let started = crate::background::spawn("Song library", move || {
        std::thread::sleep(WAIT);
        let _ = make_now(&dir, &feed);
    });
    if let Err(e) = started {
        log::warning("Song library", &format!("Couldn't start making songs.db: {e}"));
    }
}

/// Makes `songs.db` in `dir` now, saying how on `feed` and in the log: what it made, or why it couldn't.
pub fn make_now(dir: &Path, feed: &Feed) -> Result<Made, String> {
    let library = dir.join(songs_db::OLD_FILE_NAME);
    let songs = dir.join(songs_db::FILE_NAME);
    let job = feed.start("Making the app's own song library", "Reading library.db once, to make songs.db from it");
    let began = Instant::now();
    let made = room_for(&library).and_then(|()| {
        songs_db::make(&library, &songs, &mut |done| job.progress(done, None)).map_err(|e| e.to_string())
    });
    match &made {
        Ok(made) => {
            let line = made_line(made);
            log::info(
                "Song library",
                &format!(
                    "Made songs.db in {:.1} s ({} rows checked): {line}",
                    began.elapsed().as_secs_f32(),
                    made.rows
                ),
            );
            job.done_as("Made the app's own song library", &line);
        }
        Err(why) => {
            log::warning("Song library", &format!("Couldn't make songs.db: {why}"));
            job.failed_as("Couldn't make the app's own song library", &format!("{why}. It's tried again next start."));
        }
    }
    made
}

/// What the card says once it's made: "3,041 saves, 96 MB beside library.db's 720 MB. Songs opens from it at the
/// next start."
pub fn made_line(made: &Made) -> String {
    let saves = if made.saves == 1 { "1 save".to_owned() } else { format!("{} saves", thousands(made.saves as usize)) };
    format!(
        "{saves}, {} beside library.db's {}. The app opens it from the next start.",
        mo_space::size_text(made.bytes),
        mo_space::size_text(made.from_bytes)
    )
}

/// Whether the drive has room for the new file: about half of `library.db` (the track details compress to a fraction
/// of it) and [`SPARE_ROOM`] to spare.
fn room_for(library: &Path) -> Result<(), String> {
    let size = std::fs::metadata(library).map_or(0, |m| m.len());
    let needed = size / 2 + SPARE_ROOM;
    let free = library.ancestors().find(|p| p.exists()).and_then(|p| fs4::available_space(p).ok());
    match free {
        Some(free) if free < needed => Err(format!(
            "there isn't room on the drive: it needs about {} and has {} free",
            mo_space::size_text(needed),
            mo_space::size_text(free)
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_says_what_was_made_in_plain_words() {
        let made = Made { saves: 3_041, rows: 400_000, bytes: 96 << 20, from_bytes: 720 << 20 };
        let line = made_line(&made);
        assert!(line.starts_with("3,041 saves, "), "{line}");
        assert!(line.ends_with("The app opens it from the next start."), "{line}");
        assert!(made_line(&Made { saves: 1, ..made }).starts_with("1 save, "));
    }

    #[test]
    fn made_now_from_the_library_beside_it_and_said_on_a_card() {
        let dir = std::env::temp_dir().join(format!("mo-song-library-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        rusqlite::Connection::open(dir.join("library.db"))
            .unwrap()
            .execute_batch(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                     name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, mtime INTEGER, info TEXT);
                 INSERT INTO projects VALUES (1, 'C:\\a\\One.flp', 'One', 'FL Studio', 'C:\\a', 1, '{}');",
            )
            .unwrap();
        let feed = Feed::default();
        let made = make_now(&dir, &feed).unwrap();
        assert_eq!(made.saves, 1);
        let cards = feed.visible(Instant::now());
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "Made the app's own song library");
        // Made once: the next start switches to it.
        assert_eq!(songs_db::at_start(&dir).library, dir.join("songs.db"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
