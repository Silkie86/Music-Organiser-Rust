//! Files the backup keeps that are gone from your folders: deleted, or moved, since they were backed up. Solomon,
//! 3 Nov 2026: a file deleted from your music stays on the backup, and the app says so, so a file you didn't mean to
//! delete can still be copied back. Nothing here removes or changes anything.
//!
//! [`find`] goes through everything the backup holds (the folder backup's [record](crate::record) and the list inside
//! each song zip, [`crate::pack`]) and asks the system about each file that wasn't among your files when they were
//! last walked. A file only counts as gone when the system says nothing is there (not when it can't be read, or a walk
//! left it out) and the nearest folder above it that's there is a real folder. One behind a link whose place has gone,
//! on a drive that isn't plugged in, or on a network place that can't be reached can't be told from one that's gone, so
//! it isn't counted. (On a Mac or Linux a drive that isn't plugged in can leave nothing where it was mounted, so its
//! files can read as gone there: it's only ever a warning.) And only while the backup still has it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use crate::pack;
use crate::plan::{from_backup, place_on_backup};
use crate::record::{self, Record};

/// A file the backup keeps that's gone from your folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gone {
    /// Where it was.
    pub was: PathBuf,
    pub size: u64,
    /// The file's own time when it was backed up, in nanoseconds since 1970.
    pub time: i128,
    /// Where the backup keeps it: a copy in its folders, a song's zip, or several of those (a sample two songs use).
    pub kept: Vec<Kept>,
}

/// One place on the backup that keeps a gone file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kept {
    /// A copy in the backup's folders.
    Copy(PathBuf),
    /// Inside a song's zip.
    InZip(PathBuf),
}

/// The files the backup at `backup` keeps that are gone from your folders, in order of where they were. `present` is
/// every file in your folders now, as the walk found them (and the samples outside them your songs use): only the
/// files the backup holds that aren't among them are asked about. Reads the backup's record and its zips' lists, then
/// asks the system about each file that may be gone, several at a time on the current rayon pool (call it inside the
/// app's low-priority pool, never on the window's thread). Nothing backed up yet, or no backup drive: none. A record
/// that can't be read is an error; a zip that can't be read is left out.
pub fn find(backup: &Path, present: &[&Path], stop: &AtomicBool) -> io::Result<Vec<Gone>> {
    let ignores_case = cfg!(any(windows, target_os = "macos"));
    let record = Record::load(backup, ignores_case)?;
    let zips = pack::lists(backup, stop)?;
    // The key a file is kept under: its place on the backup, relative, letter case folded where it doesn't count.
    let key_of = |path: &Path| -> Option<String> {
        let dest = place_on_backup(backup, path)?;
        record::rel_path(backup, &dest).map(|rel| record::fold(&rel, ignores_case))
    };
    let present: HashSet<String> = present.iter().filter_map(|p| key_of(p)).collect();

    // Each file the backup holds that isn't in your folders now: its place as written, size and time, and where it's
    // kept. Sorted by key, so the list comes out the same every time.
    let mut maybe: BTreeMap<String, Gone> = BTreeMap::new();
    let mut note = |key: String, rel: &str, size: u64, time: i128, kept: Kept| {
        if present.contains(&key) {
            return;
        }
        let Some(was) = from_backup(rel) else { return };
        let gone = maybe.entry(key).or_insert_with(|| Gone { was, size, time, kept: Vec::new() });
        gone.kept.push(kept);
    };
    for (key, entry) in record.entries() {
        if let Some(copy) = record::place(backup, &entry.path) {
            note(key.to_owned(), &entry.path, entry.size, entry.time, Kept::Copy(copy));
        }
    }
    for list in &zips {
        for (size, time, inside) in &list.files {
            note(record::fold(inside, ignores_case), inside, *size, *time, Kept::InZip(list.zip.clone()));
        }
    }
    if stop.load(Ordering::Relaxed) {
        return Err(stopped());
    }

    let folders = Folders::default();
    let mut gone: Vec<Gone> = maybe
        .into_par_iter()
        .filter_map(|(_, mut g)| {
            if stop.load(Ordering::Relaxed) || !folders.gone(&g.was) {
                return None;
            }
            // A copy only counts while it's still on the backup (one removed by hand isn't kept any more).
            g.kept.retain(|k| match k {
                Kept::Copy(copy) => fs::symlink_metadata(copy).is_ok_and(|m| m.is_file()),
                Kept::InZip(_) => true,
            });
            g.kept.sort();
            g.kept.dedup();
            (!g.kept.is_empty()).then_some(g)
        })
        .collect();
    if stop.load(Ordering::Relaxed) {
        return Err(stopped());
    }
    gone.sort_by(|a, b| a.was.cmp(&b.was));
    Ok(gone)
}

fn stopped() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "Stopped")
}

/// What's at a folder, for telling whether a file in it is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum At {
    /// A real folder (not a link): a file in it that the system says isn't there is gone.
    Folder,
    /// Nothing, under a real folder: everything that was in it is gone.
    Gone,
    /// Can't tell: a link, a file, a drive or network place that isn't there, or a folder that can't be looked at.
    Unsure,
}

/// Asks the system about each folder once, however many gone files were in it (several threads at a time).
#[derive(Default)]
struct Folders {
    seen: Mutex<HashMap<PathBuf, At>>,
}

impl Folders {
    /// Whether a file is gone from a place that can be seen: the system says nothing is there (a file it can't read,
    /// or one a walk left out, such as a link or a file only online, is still there), and the nearest folder above it
    /// that's there is a real folder. The drive (or network place) is asked about first, so one that isn't there costs
    /// one question, not one for each folder on it.
    fn gone(&self, was: &Path) -> bool {
        let (Some(root), Some(folder)) = (was.ancestors().last(), was.parent()) else { return false };
        if folder.as_os_str().is_empty() || self.at(root) != At::Folder {
            return false;
        }
        match self.at(folder) {
            At::Gone => true,
            At::Folder => matches!(fs::symlink_metadata(was), Err(e) if e.kind() == io::ErrorKind::NotFound),
            At::Unsure => false,
        }
    }

    fn at(&self, folder: &Path) -> At {
        if let Some(at) = self.lock().get(folder) {
            return *at;
        }
        let at = match fs::symlink_metadata(folder) {
            // Not through a link: `is_dir` is only true for a real folder.
            Ok(meta) if meta.is_dir() => At::Folder,
            Ok(_) => At::Unsure,
            Err(e) if e.kind() == io::ErrorKind::NotFound => match folder.parent() {
                Some(up) if !up.as_os_str().is_empty() => match self.at(up) {
                    At::Folder | At::Gone => At::Gone,
                    At::Unsure => At::Unsure,
                },
                _ => At::Unsure,
            },
            Err(_) => At::Unsure,
        };
        self.lock().insert(folder.to_path_buf(), at);
        at
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, At>> {
        self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
