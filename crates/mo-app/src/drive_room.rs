//! A warning, a little after the app starts, when a drive the app can't work without is almost full: the one its own
//! files are on, and the one with Windows' temp folder, where Windows and SQLite keep working files for every
//! program. On Solomon's PC drive C: ran out and reading MIDI failed with "database or disk is full" (his log,
//! 2026-11-11), which says neither which drive nor what to do. The Space page says the same of the app's own drive,
//! but only when it's looked at.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mo_space::app_files::VERY_LOW_SPACE;
use mo_space::size::size_text;

use crate::activity::Feed;
use crate::log;

/// How long after the start it looks: once the pages have read what they show.
const WAIT: Duration = Duration::from_secs(20);

/// A drive that's almost full: the card's title and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    pub title: String,
    pub detail: String,
}

/// The warnings for the drive of the app's own files (`data`) and the one of the temp folder (`temp`), given how much
/// room `free` finds at each: one for each drive with under [`VERY_LOW_SPACE`], one only when both are on the same
/// drive. A drive whose room can't be asked gets none.
pub fn warnings(data: &Path, temp: &Path, free: &dyn Fn(&Path) -> Option<u64>) -> Vec<Warning> {
    let same = drive(data) == drive(temp);
    let low = |at: &Path| free(at).filter(|room| *room < VERY_LOW_SPACE);
    let warning = |at: &Path, room: u64, holds: &str| Warning {
        title: format!("{} is almost full", drive(at)),
        detail: format!(
            "Only {} is free on it, and it holds {holds}. Reading and saving in the app can fail until there's more \
             room: the Space page shows what takes the most.",
            size_text(room)
        ),
    };
    let mut found = Vec::new();
    if let Some(room) = low(temp) {
        let holds = if same {
            "the app's own files and the working files of Windows and every program"
        } else {
            "the working files of Windows and every program, this app's too"
        };
        found.push(warning(temp, room, holds));
    }
    if !same && let Some(room) = low(data) {
        found.push(warning(data, room, "the app's own files"));
    }
    found
}

/// The drive `path` is on, as the card names it: "Drive C:", or the folder itself where there are no drive letters.
fn drive(path: &Path) -> String {
    match path.components().next() {
        Some(std::path::Component::Prefix(letter)) => {
            format!("Drive {}", letter.as_os_str().to_string_lossy().to_uppercase())
        }
        _ => format!("The drive of {}", path.display()),
    }
}

/// Looks [`WAIT`] after the start, on a low-priority thread of its own, and puts a card that stays on `feed` for each
/// drive that's almost full (and a line in the log).
pub fn warn_later(data: PathBuf, feed: Feed) {
    let started = crate::background::spawn("Drive room", move || {
        std::thread::sleep(WAIT);
        for warning in warnings(&data, &std::env::temp_dir(), &|at| fs4::available_space(at).ok()) {
            feed.notice(&warning.title, &warning.detail);
        }
    });
    if let Err(e) = started {
        log::warning("Drive room", &format!("Couldn't start looking at the drives' room: {e}"));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    #[test]
    fn a_drive_with_room_gets_no_warning_and_an_almost_full_one_gets_one() {
        let (data, temp) = (Path::new("/data/MusicOrganiser"), Path::new("/tmp"));
        assert_eq!(warnings(data, temp, &|_| Some(50 * GB)), []);
        assert_eq!(warnings(data, temp, &|_| Some(VERY_LOW_SPACE)), []);
        // Room that can't be asked is no warning.
        assert_eq!(warnings(data, temp, &|_| None), []);
        // The temp folder's drive alone.
        let found = warnings(data, temp, &|at| Some(if at == temp { 300 << 20 } else { 50 * GB }));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].detail.starts_with("Only 300 MB is free on it, and it holds the working files"), "{found:?}");
        // Both on one drive: said once, for both uses.
        let found = warnings(temp, temp, &|_| Some(GB));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].detail.contains("the app's own files and the working files"), "{found:?}");
    }

    #[cfg(windows)]
    #[test]
    fn each_drive_is_named_by_its_letter_and_warned_of_apart() {
        let (data, temp) = (Path::new(r"I:\MusicOrganiser"), Path::new(r"c:\Users\x\AppData\Local\Temp"));
        let found = warnings(data, temp, &|_| Some(GB));
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].title, "Drive C: is almost full");
        assert_eq!(found[1].title, "Drive I: is almost full");
        assert!(found[1].detail.contains("it holds the app's own files."), "{found:?}");
    }
}
