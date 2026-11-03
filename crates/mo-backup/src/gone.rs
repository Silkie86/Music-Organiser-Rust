//! Files the backup keeps that are gone from your folders: deleted, or moved, since they were backed up. Solomon,
//! 3 Nov 2026: a file deleted from your music stays on the backup, and the app says so, so a file you didn't mean to
//! delete can still be copied back. Nothing here removes or changes anything.
//!
//! [`find`] goes through everything the backup holds (the folder backup's [record](crate::record) and the list inside
//! each song zip, [`crate::pack`]) and asks the system about each file that wasn't among your files when they were
//! last walked. A file only counts as gone when the system says nothing is there (not when it can't be read, or a walk
//! left it out), the nearest folder above it that's there is a real folder, and its drive is there. What can't be told
//! from gone isn't counted:
//!
//! - a file behind a link whose place has gone;
//! - one on a drive that isn't plugged in, or a network place that can't be reached (away from Windows, a drive
//!   counts only while something is mounted where it goes: `/Volumes/<drive>`, `/media/<you>/<drive>`, `/mnt/<drive>`);
//! - one in a folder on your backup's list that isn't there now (the page says it isn't plugged in);
//! - one on a drive where such a folder was and none of your files are now: another drive may have taken its letter.
//!
//! And only while the backup still has it. What the backup holds is kept in [`Holdings`] between looks, so a look only
//! reads again its record, or a song zip, when that changed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use rayon::prelude::*;

use crate::pack::{self, Line, ZipList};
use crate::plan::{from_backup, place_on_backup};
use crate::record::{self, Record};
use crate::stopped;

/// A file the backup keeps that's gone from your folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gone {
    /// Where it was.
    pub was: PathBuf,
    /// Its size and its own time (in nanoseconds since 1970) when it was backed up: the newest of the versions the
    /// backup keeps, when it keeps more than one (two song zips packed at different times).
    pub size: u64,
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

/// One version the backup keeps of a file: where, and the file's size and time then.
type Version = (Kept, u64, i128);

/// A file's size and time, to tell whether it changed since it was read.
pub(crate) type Stamp = (u64, SystemTime);

/// A file's [`Stamp`]; `None` when the system can't say (it's read again each time then).
pub(crate) fn stamp(meta: &fs::Metadata) -> Option<Stamp> {
    meta.modified().ok().map(|t| (meta.len(), t))
}

/// What a backup holds, as last read: its record's places and every song zip's list, each with the size and time of the
/// file it was read from. Kept between looks (the Backup page keeps one), so a look reads again only the record or a
/// zip that changed since; a look at another backup starts again.
#[derive(Default)]
pub struct Holdings {
    backup: PathBuf,
    /// The record's stamp when it was read (`None`: read again next time), and its places as lines (size, time,
    /// place).
    record: (Option<Stamp>, Vec<Line>),
    /// Every song zip's list, in order of the zips.
    zips: Vec<ZipList>,
}

impl Holdings {
    /// Brings what's held up to date with the backup at `backup`.
    fn refresh(&mut self, backup: &Path, ignores_case: bool, stop: &AtomicBool) -> io::Result<()> {
        if self.backup != backup {
            *self = Holdings { backup: backup.to_path_buf(), ..Holdings::default() };
        }
        match fs::metadata(record::file_on(backup)) {
            // Nothing backed up in folders yet.
            Err(e) if e.kind() == io::ErrorKind::NotFound => self.record = (None, Vec::new()),
            Err(e) => return Err(e),
            Ok(meta) => {
                let now = stamp(&meta);
                if now.is_none() || self.record.0 != now {
                    let read = Record::load(backup, ignores_case)?;
                    self.record = (now, read.entries().map(|(_, e)| (e.size, e.time, e.path.clone())).collect());
                }
            }
        }
        self.zips = pack::lists_reusing(backup, std::mem::take(&mut self.zips), stop)?;
        Ok(())
    }
}

/// The files the backup at `backup` keeps that are gone from your folders, in order of where they were.
///
/// `present` is every file in your folders now, as the walk found them (and the samples outside them your songs use):
/// only the files the backup holds that aren't among them are asked about. `missing` is the folders on your backup's
/// list that aren't there now: nothing in them, or on a drive one of them was on where none of `present` is, counts.
/// `holdings` is what the last look read, brought up to date here.
///
/// Asks the system about each file that may be gone, several at a time on the current rayon pool (call it inside the
/// app's low-priority pool, never on the window's thread). Nothing backed up yet, or no backup drive: none. A record
/// that can't be read is an error; a zip that can't be read is left out.
pub fn find(
    backup: &Path,
    present: &[&Path],
    missing: &[&Path],
    holdings: &mut Holdings,
    stop: &AtomicBool,
) -> io::Result<Vec<Gone>> {
    let ignores_case = cfg!(any(windows, target_os = "macos"));
    holdings.refresh(backup, ignores_case, stop)?;
    if stop.load(Ordering::Relaxed) {
        return Err(stopped());
    }
    // The key a file is kept under: its place on the backup, relative, letter case folded where it doesn't count.
    let key_of = |path: &Path| -> Option<String> {
        let dest = place_on_backup(backup, path)?;
        record::rel_path(backup, &dest).map(|rel| record::fold(&rel, ignores_case))
    };
    let present: HashSet<String> = present.iter().filter_map(|p| key_of(p)).collect();
    // The folders on the list that aren't there, and the drives they were on where nothing of yours is now.
    let missing: Vec<String> = missing.iter().filter_map(|m| key_of(m)).collect();
    let on_drives: HashSet<&str> = present.iter().map(|k| drive_key(k)).collect();
    let swapped: HashSet<&str> =
        missing.iter().map(|m| drive_key(m)).filter(|d| !d.is_empty() && !on_drives.contains(d)).collect();
    let cant_tell = |key: &str| {
        swapped.contains(drive_key(key))
            || missing.iter().any(|m| key.strip_prefix(m.as_str()).is_some_and(|r| r.starts_with('/')))
    };

    // Each file the backup holds that isn't in your folders now: where it was, and each version kept of it (where, its
    // size and time). Sorted by key, so the list comes out the same every time; the record first, then the zips in
    // order.
    let mut maybe: BTreeMap<String, (PathBuf, Vec<Version>)> = BTreeMap::new();
    let mut note = |key: String, rel: &str, version: Version| {
        if present.contains(&key) || cant_tell(&key) {
            return;
        }
        let Some(was) = from_backup(rel) else { return };
        maybe.entry(key).or_insert_with(|| (was, Vec::new())).1.push(version);
    };
    for (size, time, rel) in &holdings.record.1 {
        if let Some(copy) = record::place(backup, rel) {
            note(record::fold(rel, ignores_case), rel, (Kept::Copy(copy), *size, *time));
        }
    }
    for list in &holdings.zips {
        for (size, time, inside) in &list.files {
            note(record::fold(inside, ignores_case), inside, (Kept::InZip(list.zip.clone()), *size, *time));
        }
    }
    if stop.load(Ordering::Relaxed) {
        return Err(stopped());
    }

    let places = Places::default();
    let mut gone: Vec<Gone> = maybe
        .into_par_iter()
        .filter_map(|(_, (was, mut versions))| {
            if stop.load(Ordering::Relaxed) || !places.gone(&was) {
                return None;
            }
            // A copy only counts while it's still on the backup (one removed by hand isn't kept any more).
            versions.retain(|(k, _, _)| match k {
                Kept::Copy(copy) => fs::symlink_metadata(copy).is_ok_and(|m| m.is_file()),
                Kept::InZip(_) => true,
            });
            // The newest version kept is the one named.
            let &(_, size, time) = versions.iter().max_by_key(|(_, _, time)| *time)?;
            let mut kept: Vec<Kept> = versions.into_iter().map(|(k, _, _)| k).collect();
            kept.sort();
            kept.dedup();
            Some(Gone { was, size, time, kept })
        })
        .collect();
    if stop.load(Ordering::Relaxed) {
        return Err(stopped());
    }
    gone.sort_by(|a, b| a.was.cmp(&b.was));
    Ok(gone)
}

/// How many of a path's first names (after its root) name the drive it's on, away from Windows: a drive mounted at
/// `/Volumes/<drive>` (a Mac), `/media/<you>/<drive>` or `/run/media/<you>/<drive>` (Linux) or `/mnt/<drive>`. None
/// for a file on the system's own drive.
fn mount_depth(names: &[&OsStr]) -> usize {
    let is = |name: &OsStr, wanted: &str| name.to_str().is_some_and(|n| n.eq_ignore_ascii_case(wanted));
    match names {
        [volumes, _, ..] if is(volumes, "Volumes") => 2,
        [media, _, _, ..] if is(media, "media") => 3,
        [run, media, _, _, ..] if is(run, "run") && is(media, "media") => 4,
        [mnt, _, ..] if is(mnt, "mnt") => 2,
        _ => 0,
    }
}

/// The part of a key (a place on the backup, relative) that names the drive: the drive letter, a network place
/// (`Network/server/share`) or a mounted drive away from Windows; empty for the system's own drive, which can't be
/// unplugged.
fn drive_key(key: &str) -> &str {
    let names: Vec<&str> = key.split('/').collect();
    let depth = if cfg!(windows) {
        match names.first() {
            Some(first) if first.eq_ignore_ascii_case("Network") => 3,
            Some(_) => 1,
            None => 0,
        }
    } else {
        mount_depth(&names.iter().map(OsStr::new).collect::<Vec<_>>())
    };
    if depth == 0 || names.len() <= depth {
        return "";
    }
    let end = names[..depth].iter().map(|n| n.len()).sum::<usize>() + depth - 1;
    key.get(..end).unwrap_or("")
}

/// The drive a path is on: its drive letter or network place on Windows; a mounted drive (see [`mount_depth`]) or the
/// system's own (`/`) elsewhere.
fn drive_root(path: &Path) -> Option<PathBuf> {
    if cfg!(windows) {
        return path.ancestors().last().map(Path::to_path_buf);
    }
    let names: Vec<&OsStr> =
        path.components().filter_map(|c| if let Component::Normal(n) = c { Some(n) } else { None }).collect();
    let mut root = PathBuf::from("/");
    root.extend(&names[..mount_depth(&names).min(names.len())]);
    Some(root)
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

/// Asks the system about places, about each drive and each folder once however many files were in it (several
/// threads at a time): a drive that isn't there costs one question, not one for each file on it.
#[derive(Default)]
pub(crate) struct Places {
    folders: Mutex<HashMap<PathBuf, At>>,
    drives: Mutex<HashMap<PathBuf, bool>>,
}

impl Places {
    /// Whether a file is gone from a place that can be seen: the system says nothing is there (a file it can't read,
    /// or one a walk left out, such as a link or a file only online, is still there), the nearest folder above it
    /// that's there is a real folder, and its drive is there.
    pub(crate) fn gone(&self, was: &Path) -> bool {
        let (Some(drive), Some(folder)) = (drive_root(was), was.parent()) else { return false };
        if folder.as_os_str().is_empty() || !self.drive_there(&drive) {
            return false;
        }
        match self.at(folder) {
            At::Gone => true,
            At::Folder => matches!(fs::symlink_metadata(was), Err(e) if e.kind() == io::ErrorKind::NotFound),
            At::Unsure => false,
        }
    }

    /// Whether a file is there: its drive is there, and the system says a file is at that place (through links too).
    pub(crate) fn there(&self, path: &Path) -> bool {
        drive_root(path).is_some_and(|drive| self.drive_there(&drive)) && fs::metadata(path).is_ok_and(|m| m.is_file())
    }

    /// Whether a drive is there: a real folder, and away from Windows, a drive mounted on it (not just the empty
    /// folder a drive that isn't plugged in can leave).
    fn drive_there(&self, drive: &Path) -> bool {
        if let Some(there) = lock(&self.drives).get(drive) {
            return *there;
        }
        let there = fs::symlink_metadata(drive).is_ok_and(|m| m.is_dir()) && mounted(drive);
        lock(&self.drives).insert(drive.to_path_buf(), there);
        there
    }

    fn at(&self, folder: &Path) -> At {
        if let Some(at) = lock(&self.folders).get(folder) {
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
        lock(&self.folders).insert(folder.to_path_buf(), at);
        at
    }
}

/// Whether a drive is mounted on a folder: always on Windows, and for the system's own drive; elsewhere, when the
/// folder is on another device than the folder it's in.
fn mounted(drive: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Some(up) = drive.parent() else { return true };
        match (fs::symlink_metadata(drive), fs::symlink_metadata(up)) {
            (Ok(here), Ok(above)) => here.dev() != above.dev(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = drive;
        true
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drive_is_told_from_the_path() {
        if cfg!(windows) {
            assert_eq!(drive_key("i/samples/kick.wav"), "i");
            assert_eq!(drive_key("network/nas/music/kick.wav"), "network/nas/music");
            assert_eq!(drive_key("network/nas"), "");
        } else {
            assert_eq!(drive_key("Volumes/Samples/Kicks/kick.wav"), "Volumes/Samples");
            assert_eq!(drive_key("volumes/samples/kick.wav"), "volumes/samples");
            assert_eq!(drive_key("media/solomon/USB/kick.wav"), "media/solomon/USB");
            assert_eq!(drive_key("run/media/solomon/USB/kick.wav"), "run/media/solomon/USB");
            assert_eq!(drive_key("mnt/music/kick.wav"), "mnt/music");
            // The system's own drive, which can't be unplugged.
            assert_eq!(drive_key("home/solomon/Music/kick.wav"), "");
            assert_eq!(drive_key("Volumes/Samples"), "", "the drive itself, not a file on it");
            assert_eq!(
                drive_root(Path::new("/Volumes/Samples/Kicks/kick.wav")),
                Some(PathBuf::from("/Volumes/Samples"))
            );
            assert_eq!(drive_root(Path::new("/home/solomon/kick.wav")), Some(PathBuf::from("/")));
        }
    }
}
