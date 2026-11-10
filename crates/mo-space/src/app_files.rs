//! How much room the app's own files take, and whether the drive they're on is nearly full (Solomon's F12 note, 27
//! Oct: "add way to see the size of the database the app uses to hold values and make sure it warns you if the disk its
//! on is nearly full"). The C# app's `AppFiles`. The library, sounds and MIDI databases grow with every scan, and a
//! database can't save anything once its drive is full.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::size::{self, Stopped};
use crate::{key, really};

/// Under this much room left, the drive counts as nearly full.
pub const LOW_SPACE: u64 = 10 << 30;

/// Under this much, saving may already fail.
pub const VERY_LOW_SPACE: u64 = 2 << 30;

/// Whether the drive is nearly full: under [`LOW_SPACE`] free, or under what the app's own files take (a library that
/// size can grow by as much again in a big scan).
pub fn is_low(free: u64, app_bytes: u64) -> bool {
    free < LOW_SPACE.max(app_bytes)
}

/// One of the app's databases, or a copy or cache it keeps beside them: its part's name, what it holds, and its file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDatabase {
    pub name: &'static str,
    pub what: &'static str,
    pub path: PathBuf,
}

/// The three databases the app reads, by where they are.
pub fn databases(library: PathBuf, sounds: PathBuf, midi: PathBuf) -> Vec<AppDatabase> {
    vec![
        AppDatabase { name: "Your library", what: "Projects, plugins, samples, tags and ratings", path: library },
        AppDatabase { name: "Sound Lab", what: "Every sound's details, likes and look-alikes", path: sounds },
        AppDatabase { name: "MIDI parts", what: "The MIDI read from your projects and MIDI files", path: midi },
    ]
}

/// What the part with the app's own song library is called while the app still reads `library.db`
/// ([`new_song_library`]).
pub const NEW_SONG_LIBRARY: &str = "Your new song library";

/// The app's own song library at `songs` (`songs.db`), as a part of its own while the app still reads `library.db`:
/// it's made from it in the background, and opened from a later start. Listed after [`databases`], so once it's the
/// library the app reads it counts there: each file counts once ([`measure`]). Before, it was in "Everything else:
/// Your Bitwig template and the rest" until then.
pub fn new_song_library(songs: PathBuf) -> AppDatabase {
    AppDatabase {
        name: NEW_SONG_LIBRARY,
        what: "Made from library.db, for the app to open instead once it's ready",
        path: songs,
    }
}

/// The old app's databases, by name.
pub const OLD_DATABASE_FILES: [&str; 3] = ["library.db", "sounds.db", "midi.db"];

/// The folder in the app's folder they're put away in once the app's own have worked for a week
/// (`mo_app::old_databases`).
pub const OLD_DATABASES_FOLDER: &str = "old-databases";

/// What the part with the old app's databases is called ([`old_databases`]).
pub const OLD_DATABASES: &str = "Old app's databases";

/// The old app's databases in the app's `folder` (`library.db`, `sounds.db` and `midi.db`), as one part of their own:
/// once the app has made its own (`songs.db`, `sounds-rust.db`, `midi-index.db`), they're only kept from the old app,
/// and could be archived (the whole-app review of 6 Nov, X1-4). Listed after [`databases`], so one the app still reads
/// (its own isn't made yet) counts there instead: each file counts once ([`measure`]).
pub fn old_databases(folder: &Path) -> Vec<AppDatabase> {
    OLD_DATABASE_FILES
        .into_iter()
        .map(|file| AppDatabase { name: OLD_DATABASES, what: OLD_DATABASES_WHAT, path: folder.join(file) })
        .collect()
}

/// What the old app's databases are, on the card: the same whether they're still at the top of the app's folder or
/// put away in its `old-databases` folder ([`part_of`]), so both count as one part.
const OLD_DATABASES_WHAT: &str = "Kept from the old app: this app has its own now";

/// What the part with the sound library's backup copy is called ([`sound_lab_copies`]).
pub const SOUND_LAB_BACKUP: &str = "Sound Lab backup";

/// What the part with the damaged sound library set aside is called ([`sound_lab_copies`]).
pub const DAMAGED_SOUND_LAB: &str = "Damaged Sound Lab";

/// What the part with the app's caches is called ([`caches`]).
pub const CACHES: &str = "Caches";

/// The sound library's checked backup copy (`backup`) and the damaged library set aside when it was mended from it
/// (`damaged`), each a part of its own. The backup is about the library's size, and counted in "Everything else" it
/// read as your Bitwig template (the whole-app review of 10 Nov, T-4).
pub fn sound_lab_copies(backup: PathBuf, damaged: PathBuf) -> Vec<AppDatabase> {
    vec![
        AppDatabase {
            name: SOUND_LAB_BACKUP,
            what: "A checked copy of Sound Lab, to mend it from if it's ever damaged",
            path: backup,
        },
        AppDatabase {
            name: DAMAGED_SOUND_LAB,
            what: "Set aside when Sound Lab was mended from its backup; it goes after a week",
            path: damaged,
        },
    ]
}

/// The app's caches at `paths`, as one part: the fingerprints and lists it keeps so pages open quickly, which it makes
/// again if they go.
pub fn caches(paths: impl IntoIterator<Item = PathBuf>) -> Vec<AppDatabase> {
    let what = "Fingerprints and lists kept so pages open quickly: the app makes them again if they go";
    paths.into_iter().map(|path| AppDatabase { name: CACHES, what, path }).collect()
}

/// One part of the app's own files: a database, the search models, the logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppFilesPart {
    pub name: &'static str,
    pub what: &'static str,
    pub bytes: u64,
}

/// A database's own file, then the files SQLite keeps beside it while it's open (its write-ahead log and its index
/// of it).
pub const JOURNALS: [&str; 4] = ["", "-wal", "-shm", "-journal"];

/// Measures the databases, each with the files SQLite keeps beside it ("library.db-wal"), then the rest of the app's
/// `folder` by what each thing at its top holds, biggest first. Links are left out, as everywhere sizes are measured.
pub fn measure(folder: &Path, databases: &[AppDatabase], stop: &AtomicBool) -> Result<Vec<AppFilesPart>, Stopped> {
    let mut parts: BTreeMap<&'static str, (&'static str, u64)> = BTreeMap::new();
    let mut add = |name: &'static str, what: &'static str, bytes: u64| {
        let part = parts.entry(name).or_insert((what, 0));
        part.1 = part.1.saturating_add(bytes);
    };
    let mut counted: HashSet<String> = HashSet::new();
    for db in databases {
        for journal in JOURNALS {
            let mut file = db.path.clone().into_os_string();
            file.push(journal);
            let file = PathBuf::from(file);
            if counted.insert(key(&file.to_string_lossy()))
                && let Ok(meta) = fs::symlink_metadata(&file)
                && meta.is_file()
            {
                add(db.name, db.what, meta.len());
            }
        }
    }
    // A folder that can't be read still leaves the databases' sizes.
    let entries = fs::read_dir(folder).into_iter().flatten().flatten();
    for entry in entries {
        if stop.load(Ordering::Relaxed) {
            return Err(Stopped);
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_symlink() || counted.contains(&key(&path.to_string_lossy())) {
            continue;
        }
        let half_made = if kind.is_dir() { None } else { half_made_part(&path, databases) };
        let (name, what) = half_made.unwrap_or_else(|| part_of(&entry.file_name().to_string_lossy(), kind.is_dir()));
        let bytes = if kind.is_dir() {
            size::measure(&path, stop)?.bytes
        } else {
            entry.metadata().map_or(0, |m| if size::is_online_only(&m) { 0 } else { m.len() })
        };
        add(name, what, bytes);
    }
    let mut out: Vec<AppFilesPart> = parts
        .into_iter()
        .filter(|(_, (_, bytes))| *bytes > 0)
        .map(|(name, (what, bytes))| AppFilesPart { name, what, bytes })
        .collect();
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.name.cmp(b.name)));
    Ok(out)
}

/// What a new copy of a database is called while it's being made, after the database's own name and before the app's
/// process number: the sound library's copy and its backup copy ("sounds-rust.db.part-1234",
/// "sounds-rust.db.backup.part-1234"), and the song library, the MIDI index and the saves list ("songs.db.new-1234").
const BEING_MADE: [&str; 2] = [".part-", ".new-"];

/// The part of the `databases` a file at `path` counts in when it's a new copy of one being made ([`BEING_MADE`]), or
/// one of the files SQLite keeps beside it ("songs.db.new-1234-journal"): the database's own, as it will be once it's
/// made. A backup copy being made again is about the library's size, and in "Everything else" it read as your Bitwig
/// template for as long as it took.
fn half_made_part(path: &Path, databases: &[AppDatabase]) -> Option<(&'static str, &'static str)> {
    // Paths compared as Space compares them, in capitals: so are the endings.
    let ending = |end: &str| mo_formats::case::key(end);
    let file = key(&path.to_string_lossy());
    databases.iter().find_map(|db| {
        let rest = file.strip_prefix(&key(&db.path.to_string_lossy()))?;
        let number = BEING_MADE.iter().find_map(|end| rest.strip_prefix(ending(end).as_str()))?;
        let journal = number.trim_start_matches(|c: char| c.is_ascii_digit());
        let beside = JOURNALS.iter().any(|j| ending(j) == journal);
        (journal.len() < number.len() && beside).then_some((db.name, db.what))
    })
}

/// Which part a file or folder at the top of the app's folder is, and what it holds.
pub fn part_of(name: &str, folder: bool) -> (&'static str, &'static str) {
    let name = if folder { mo_formats::case::to_lower(name) } else { String::new() };
    match name.as_str() {
        "models" => ("Search models", "What finds sounds and MIDI from what you type"),
        "sounds" => ("Playground sound", "The SoundFont the Playground plays"),
        "playground" | "playground-quick" => ("Playground ideas", "Your Playground ideas and quick saves"),
        "logs" => ("Logs", "What the app did, for when something goes wrong"),
        "notes" => ("Notes for Claude", "What you wrote with F12, with its pictures"),
        "reports" | "probe" => ("Test reports", "Live tests, self-checks and MO Probe"),
        OLD_DATABASES_FOLDER => (OLD_DATABASES, OLD_DATABASES_WHAT),
        _ => ("Everything else", "Your Bitwig template and the rest"),
    }
}

/// The drive a file or folder is on, its room left and its size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveSpace {
    /// "C:" on Windows; "your disk" elsewhere.
    pub drive: String,
    pub free: u64,
    pub total: u64,
}

/// How much room is left on the drive `path` is on; `None` when that can't be read.
pub fn drive_space(path: &Path) -> Option<DriveSpace> {
    // Where it really is: Windows finds the drive from the path's text, so a link on the way would give another drive.
    let real = really(path);
    let free = fs4::available_space(&real).ok()?;
    let total = fs4::total_space(&real).ok()?;
    // A mapped network drive keeps the letter you know rather than the share's name.
    let named = if real.to_string_lossy().starts_with(r"\\") { path } else { &real };
    Some(DriveSpace { drive: drive_name(named), free, total })
}

fn drive_name(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match absolute.components().next() {
        Some(std::path::Component::Prefix(prefix)) => prefix.as_os_str().to_string_lossy().into_owned(),
        _ => "your disk".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearly_full_is_under_ten_gigabytes_or_under_the_apps_own_size() {
        assert!(is_low(5 << 30, 1 << 30));
        assert!(!is_low(20 << 30, 1 << 30));
        assert!(is_low(20 << 30, 30 << 30));
    }

    #[test]
    fn top_folders_are_named_parts() {
        assert_eq!(part_of("Models", true).0, "Search models");
        assert_eq!(part_of("logs", false).0, "Everything else");
        assert_eq!(part_of("playground-quick", true).0, "Playground ideas");
    }
}
