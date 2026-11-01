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

/// One of the app's databases: what it's called on Settings, what it holds, and its file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDatabase {
    pub name: &'static str,
    pub what: &'static str,
    pub path: PathBuf,
}

/// The three databases, by where they are.
pub fn databases(library: PathBuf, sounds: PathBuf, midi: PathBuf) -> Vec<AppDatabase> {
    vec![
        AppDatabase { name: "Your library", what: "Projects, plugins, samples, tags and ratings", path: library },
        AppDatabase { name: "Sound Lab", what: "Every sound's details, likes and look-alikes", path: sounds },
        AppDatabase { name: "MIDI parts", what: "The MIDI read from your projects and MIDI files", path: midi },
    ]
}

/// One part of the app's own files: a database, the search models, the logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppFilesPart {
    pub name: &'static str,
    pub what: &'static str,
    pub bytes: u64,
}

/// The files SQLite keeps beside a database while it's open (its write-ahead log and its index of it).
const JOURNALS: [&str; 4] = ["", "-wal", "-shm", "-journal"];

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
        let (name, what) = part_of(&entry.file_name().to_string_lossy(), kind.is_dir());
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
