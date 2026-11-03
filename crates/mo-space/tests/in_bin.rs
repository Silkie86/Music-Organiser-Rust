//! The reminder of what the app has put in the Recycle Bin that's still there. The bins here are made up: no test
//! looks in the computer's own bin.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use mo_space::clean::{Bin, Done, Problem, Removed, What};
use mo_space::in_bin::{InBin, remind};
use mo_space::plugin_space::SpaceKind;
use mo_space::song::CompactKind;

/// A bin holding the things whose bin id it's given, or one that can't be looked in.
struct MadeUpBin {
    holding: Option<&'static [&'static str]>,
}

impl Bin for MadeUpBin {
    fn send(&self, _path: &Path) -> Result<(), String> {
        Ok(())
    }
    fn find(&self, _removed: &mut [Removed]) -> Vec<Problem> {
        Vec::new()
    }
    fn put_back(&self, _removed: &[Removed]) -> Vec<Problem> {
        Vec::new()
    }
    fn holds(&self, removed: &[Removed]) -> Option<Vec<bool>> {
        let holding = self.holding?;
        Some(removed.iter().map(|r| r.bin_id.as_deref().is_some_and(|id| holding.contains(&id))).collect())
    }
}

/// A day's Unix seconds at noon, the computer's own time, so it's that day wherever the test runs.
fn noon(day: &str) -> i64 {
    let at = chrono::NaiveDateTime::parse_from_str(&format!("{day} 12:00"), "%Y-%m-%d %H:%M").unwrap();
    at.and_local_timezone(chrono::Local).earliest().unwrap().timestamp()
}

fn removed(real: &str, bytes: u64, day: &str, bin_id: &str) -> Removed {
    Removed {
        path: real.to_owned(),
        real: real.to_owned(),
        bytes,
        what: What::Song(CompactKind::Backup),
        when: noon(day),
        bin_id: Some(bin_id.to_owned()),
    }
}

const MB: u64 = 1 << 20;

fn done(removed: Vec<Removed>) -> Done {
    Done { removed, ..Done::default() }
}

fn bin_word() -> &'static str {
    if cfg!(windows) { "Recycle Bin" } else { "Trash" }
}

#[test]
fn what_the_app_put_in_the_bin_and_is_still_there_is_added_up_by_drive() {
    let songs = done(vec![
        removed(r"C:\Music\Song\Backup\Song 1.flp", MB, "2026-10-20", "a"),
        // Put back, or emptied from the bin, since: it doesn't count.
        removed(r"C:\Music\Song\Backup\Song 2.flp", 9 * MB, "2026-10-01", "b"),
    ]);
    let mut plugin = removed(r"\\?\d:\Plugins\Old Presets", 3 * MB, "2026-11-02", "c");
    plugin.what = What::Plugin(SpaceKind::Presets);
    let plugins = done(vec![plugin, removed(r"D:\Plugins\Kontakt Old", MB / 2, "2026-11-03", "d")]);
    let bin = MadeUpBin { holding: Some(&["a", "c", "d"]) };

    let found = remind(&[songs, plugins], &bin);
    assert_eq!(
        found,
        InBin {
            items: 3,
            bytes: 4 * MB + MB / 2,
            since: Some(noon("2026-10-20")),
            drives: vec![("D:".to_owned(), 3 * MB + MB / 2), ("C:".to_owned(), MB)],
            looked: true,
        }
    );
    assert_eq!(found.title(), format!("4.5 MB the app put in the {} is still there", bin_word()));
    assert_eq!(
        found.note(),
        format!(
            "3 files and folders, since 20 Oct 2026. It still takes room on D: (3.5 MB) and C: (1 MB) until you \
             empty the {}: the app never empties it, so until then you can put it back.",
            bin_word()
        )
    );
}

#[test]
fn one_thing_on_one_drive_is_said_so() {
    let found = remind(
        &[done(vec![removed(r"C:\Music\Song\Cache", 2_048, "2026-11-03", "a")])],
        &MadeUpBin { holding: Some(&["a"]) },
    );
    assert_eq!(
        found.note(),
        format!(
            "1 file or folder, since 3 Nov 2026. It still takes room on C: until you empty the {}: the app never \
             empties it, so until then you can put it back.",
            bin_word()
        )
    );
}

#[test]
fn a_bin_that_cant_be_looked_in_counts_all_the_app_put_there() {
    // A Mac's Trash: what's in it can't be told, so it's all of it, and it's said as what the app put there.
    let found = remind(
        &[done(vec![removed("/Users/solomon/Music/Song/Backup/Song 1.als", MB + MB / 2, "2026-11-01", "a")])],
        &MadeUpBin { holding: None },
    );
    assert_eq!((found.items, found.bytes, found.looked), (1, MB + MB / 2, false));
    // The paths don't name drives.
    assert!(found.drives.is_empty());
    assert_eq!(found.title(), format!("The app has put 1.5 MB in the {}", bin_word()));
    assert!(found.note().starts_with("1 file or folder, since 1 Nov 2026. It still takes room until you empty"));
}

#[test]
fn nothing_left_in_the_bin_is_nothing_to_remind_you_of() {
    let emptied = remind(
        &[done(vec![removed(r"C:\Music\Song\Backup\Song 1.flp", 1_000, "2026-10-20", "a")])],
        &MadeUpBin { holding: Some(&[]) },
    );
    assert!(emptied.is_empty() && emptied.looked);
    assert_eq!((emptied.title(), emptied.note()), (String::new(), String::new()));
    assert!(remind(&[], &MadeUpBin { holding: None }).is_empty());
}
