//! A warning, a little after the app starts, when a drive the app can't work without is almost full: the one its own
//! files are on, and the one with Windows' temp folder, where Windows and SQLite keep working files for every
//! program. On Solomon's PC drive C: ran out and reading MIDI failed with "database or disk is full" (his log,
//! 2026-11-11), which says neither which drive nor what to do. The Space page says the same of the app's own drive,
//! but only when it's looked at.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mo_space::app_files::{DriveSpace, VERY_LOW_SPACE, drive_space};
use mo_space::size::size_text;

use crate::activity::Feed;
use crate::log;

/// How long after the start it first looks: once the pages have read what they show.
const WAIT: Duration = Duration::from_secs(20);
/// How often it looks again: a drive can fill up while the app is open (another program, a long scan).
const AGAIN: Duration = Duration::from_secs(10 * 60);

/// A drive that's almost full: its name ("C:"), the card's title and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    pub drive: String,
    pub title: String,
    pub detail: String,
}

/// The warnings for the drive of the app's own files (`data`) and the one of the temp folder (`temp`), given what
/// `look` finds of each one's drive ([`drive_space`]: its name and room, where the folder really is): one for each
/// drive with under [`VERY_LOW_SPACE`] free, one only when both are on the same drive. A drive that can't be looked
/// at gets none.
pub fn warnings(data: &Path, temp: &Path, look: &dyn Fn(&Path) -> Option<DriveSpace>) -> Vec<Warning> {
    let (data, temp) = (look(data), look(temp));
    // The same drive has the same name and size (and "your disk", where there are no letters, the same size).
    let same = matches!((&data, &temp), (Some(d), Some(t)) if d.drive == t.drive && d.total == t.total);
    let warning = |at: &DriveSpace, holds: &str, then: &str| Warning {
        drive: at.drive.clone(),
        title: format!("{} is almost full", named(&at.drive)),
        detail: format!(
            "Only {} is free on it, and it holds {holds}. Reading and saving in the app can fail until there's more \
             room{then}.",
            size_text(at.free)
        ),
    };
    let low = |at: &Option<DriveSpace>| at.clone().filter(|d| d.free < VERY_LOW_SPACE);
    const SPACE_PAGE: &str = ": the Space page shows what of yours takes the most";
    let mut found = Vec::new();
    if let Some(at) = low(&temp) {
        found.push(if same {
            warning(&at, "the app's own files and the working files of every program", SPACE_PAGE)
        } else {
            warning(&at, "the working files of every program, this app's too", "")
        });
    }
    if !same && let Some(at) = low(&data) {
        found.push(warning(&at, "the app's own files", SPACE_PAGE));
    }
    found
}

/// A drive as the card's title names it: "Drive C:", or "Your disk" where there are no drive letters.
fn named(drive: &str) -> String {
    if drive.ends_with(':') {
        return format!("Drive {}", drive.to_uppercase());
    }
    let mut letters = drive.chars();
    letters.next().map_or_else(String::new, |first| first.to_uppercase().chain(letters).collect())
}

/// Looks [`WAIT`] after the start and every [`AGAIN`] after, on a low-priority thread of its own, and puts a card
/// that stays on `feed` for each drive that's almost full (and a line in the log): once a run for each drive.
pub fn watch(data: PathBuf, feed: Feed) {
    let started = crate::background::spawn("Drive room", move || {
        let mut told = HashSet::new();
        std::thread::sleep(WAIT);
        loop {
            for warning in warnings(&data, &std::env::temp_dir(), &drive_space) {
                if told.insert(warning.drive.clone()) {
                    feed.notice(&warning.title, &warning.detail);
                }
            }
            std::thread::sleep(AGAIN);
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

    fn at(drive: &str, free: u64, total: u64) -> Option<DriveSpace> {
        Some(DriveSpace { drive: drive.to_owned(), free, total })
    }

    #[test]
    fn a_drive_with_room_gets_no_warning_and_an_almost_full_one_gets_one() {
        let (data, temp) = (Path::new("data"), Path::new("temp"));
        let two = |c: u64, i: u64| {
            warnings(data, temp, &|p| if p == temp { at("c:", c, 500 * GB) } else { at("I:", i, 2000 * GB) })
        };
        assert_eq!(two(50 * GB, 50 * GB), []);
        assert_eq!(two(VERY_LOW_SPACE, VERY_LOW_SPACE), []);
        // A drive that can't be looked at is no warning.
        assert_eq!(warnings(data, temp, &|_| None), []);
        // The temp folder's drive alone (Solomon's PC): named, and not sent to the Space page, which shows the app's.
        let found = two(300 << 20, 50 * GB);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].title, "Drive C: is almost full");
        assert_eq!(
            found[0].detail,
            "Only 300 MB is free on it, and it holds the working files of every program, this app's too. Reading and \
             saving in the app can fail until there's more room."
        );
        // Both drives: one each, the app's own with where to look.
        let found = two(GB, GB);
        assert_eq!(
            found.iter().map(|w| w.title.as_str()).collect::<Vec<_>>(),
            ["Drive C: is almost full", "Drive I: is almost full"]
        );
        assert!(found[1].detail.contains("it holds the app's own files. "), "{found:?}");
        assert!(found[1].detail.ends_with("the Space page shows what of yours takes the most."), "{found:?}");
        // Both folders on one drive: said once, for both uses. Without letters too.
        let found = warnings(data, temp, &|_| at("C:", GB, 500 * GB));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].detail.contains("the app's own files and the working files"), "{found:?}");
        let found = warnings(data, temp, &|_| at("your disk", GB, 500 * GB));
        assert_eq!(found.iter().map(|w| w.title.as_str()).collect::<Vec<_>>(), ["Your disk is almost full"]);
    }
}
