//! Which date of a sound the Sounds page's year strip goes by (Solomon, 13 Nov 2026): when its file was made, last
//! changed or last opened, or when you first used it in a song. The library keeps only the date the file was last
//! changed; when the file was made and last opened are read from the sample folders when they're asked for
//! ([`read`]), each folder listed once, and kept only while the page needs them (rule 23: quick to work out, so not
//! stored).

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Local, NaiveDateTime};
use mo_sounds::Sound;
use rayon::prelude::*;

/// Which of a sound's dates the year strip goes by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DateBy {
    /// When the file was made on this computer: often the day it was copied or downloaded.
    Made,
    /// When the file was last changed: what the library has always kept (for a pack, often its maker's date).
    #[default]
    Changed,
    /// When the file was last opened. Windows often doesn't keep it on a big drive, and the app's own plays count.
    Opened,
    /// When you first used it in a song ([`mo_library::first_use`]).
    FirstUsed,
}

impl DateBy {
    pub const ALL: [DateBy; 4] = [DateBy::Made, DateBy::Changed, DateBy::Opened, DateBy::FirstUsed];

    /// Its name on the strip's menu (and in the settings).
    pub fn name(self) -> &'static str {
        match self {
            DateBy::Made => "Made",
            DateBy::Changed => "Changed",
            DateBy::Opened => "Opened",
            DateBy::FirstUsed => "First used",
        }
    }

    /// What its menu line says it means.
    pub fn about(self) -> &'static str {
        match self {
            DateBy::Made => "When the file was made on this computer (often the day you got it)",
            DateBy::Changed => "When the file was last changed",
            DateBy::Opened => "When the file was last opened (Windows often doesn't keep this, and plays count)",
            DateBy::FirstUsed => "When you first put it in a song, from your songs' saves",
        }
    }

    /// The one named `name` ([`DateBy::name`]); the default for a name it doesn't know.
    pub fn named(name: &str) -> DateBy {
        DateBy::ALL.into_iter().find(|d| d.name() == name).unwrap_or_default()
    }

    /// Whether its dates are read from the files when asked for ([`read`]).
    pub fn from_files(self) -> bool {
        matches!(self, DateBy::Made | DateBy::Opened)
    }
}

/// When each sound's file was made and last opened, by sound id, as [`read`] found them. A sound whose file couldn't
/// be read (gone, or on a drive that isn't there), or whose system doesn't keep a date, has none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDates {
    made: HashMap<i64, NaiveDateTime>,
    opened: HashMap<i64, NaiveDateTime>,
}

impl FileDates {
    pub fn made(&self, id: i64) -> Option<NaiveDateTime> {
        self.made.get(&id).copied()
    }

    pub fn opened(&self, id: i64) -> Option<NaiveDateTime> {
        self.opened.get(&id).copied()
    }

    /// Gives the sound `id` these dates (for tests and screenshots: [`read`] finds them).
    pub fn set(&mut self, id: i64, made: Option<NaiveDateTime>, opened: Option<NaiveDateTime>) {
        for (dates, date) in [(&mut self.made, made), (&mut self.opened, opened)] {
            match date {
                Some(t) => dates.insert(id, t),
                None => dates.remove(&id),
            };
        }
    }
}

/// Reads when the files of `sounds` were made and last opened. Each folder is listed once, folders side by side on the
/// pool it's run on: on Windows a folder's listing brings every file's dates with it, so no file is opened. Slow on a
/// big library: never on the window's thread.
pub fn read(sounds: &[Sound]) -> FileDates {
    let mut by_folder: HashMap<&Path, Vec<(&std::ffi::OsStr, i64)>> = HashMap::new();
    for s in sounds {
        let path = Path::new(&s.path);
        if let (Some(folder), Some(name)) = (path.parent(), path.file_name()) {
            by_folder.entry(folder).or_default().push((name, s.id));
        }
    }
    let found: Vec<Vec<Found>> = by_folder.into_par_iter().map(|(folder, files)| in_folder(folder, &files)).collect();
    let mut dates = FileDates::default();
    for (id, made, opened) in found.into_iter().flatten() {
        if let Some(t) = made {
            dates.made.insert(id, t);
        }
        if let Some(t) = opened {
            dates.opened.insert(id, t);
        }
    }
    dates
}

/// A sound's id with when its file was made and last opened.
type Found = (i64, Option<NaiveDateTime>, Option<NaiveDateTime>);

/// The dates of `files` (name, sound id) in `folder`, from one listing of it. Names are matched as the file system
/// gives them; one the listing doesn't give as it's stored (letters in another case) is asked for by its path.
fn in_folder(folder: &Path, files: &[(&std::ffi::OsStr, i64)]) -> Vec<Found> {
    let mut listed: HashMap<std::ffi::OsString, (Option<NaiveDateTime>, Option<NaiveDateTime>)> = HashMap::new();
    // A folder of a few files is quicker asked file by file than listed whole (it may hold thousands of others).
    if files.len() > 4
        && let Ok(entries) = std::fs::read_dir(folder)
    {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                listed.insert(entry.file_name(), dates_of(&meta));
            }
        }
    }
    files
        .iter()
        .map(|&(name, id)| {
            let (made, opened) = listed
                .get(name)
                .copied()
                .unwrap_or_else(|| std::fs::metadata(folder.join(name)).map_or((None, None), |meta| dates_of(&meta)));
            (id, made, opened)
        })
        .collect()
}

fn dates_of(meta: &std::fs::Metadata) -> (Option<NaiveDateTime>, Option<NaiveDateTime>) {
    (meta.created().ok().map(local), meta.accessed().ok().map(local))
}

fn local(t: SystemTime) -> NaiveDateTime {
    DateTime::<Local>::from(t).naive_local()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound(id: i64, path: String) -> Sound {
        Sound {
            id,
            path,
            added: NaiveDateTime::default(),
            error: None,
            analysis: None,
            name_type: None,
            user_type: None,
            sound_type: "Kick".into(),
            type_source: "name".into(),
            type_confidence: 1.0,
            favourite: false,
            rating: 0,
            plays: 0,
            tags: Vec::new(),
            hidden: false,
            activity: Default::default(),
        }
    }

    #[test]
    fn every_date_by_goes_by_its_name() {
        for d in DateBy::ALL {
            assert_eq!(DateBy::named(d.name()), d);
        }
        assert_eq!(DateBy::named("Something else"), DateBy::Changed);
        assert!(DateBy::Made.from_files() && DateBy::Opened.from_files());
        assert!(!DateBy::Changed.from_files() && !DateBy::FirstUsed.from_files());
    }

    #[test]
    fn the_files_dates_are_read_from_their_folders() {
        let dir = std::env::temp_dir().join(format!("mo-app-sound-dates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut sounds = Vec::new();
        // More than a few, so the folder is listed; and one in a folder of its own, asked for by its path.
        for n in 0..6 {
            let path = dir.join(format!("{n}.wav"));
            std::fs::write(&path, b"RIFF").unwrap();
            sounds.push(sound(n, path.display().to_string()));
        }
        sounds.push(sound(50, dir.join("gone.wav").display().to_string()));
        let dates = read(&sounds);
        let now = Local::now().naive_local();
        for n in 0..6 {
            let opened = dates.opened(n).expect("every system keeps when a file was opened");
            assert!((now - opened).num_hours().abs() < 24, "{opened} for {n}");
            // Linux keeps when a file was made only on some file systems.
            if let Some(made) = dates.made(n) {
                assert!((now - made).num_hours().abs() < 24);
            }
        }
        assert_eq!((dates.made(50), dates.opened(50)), (None, None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
