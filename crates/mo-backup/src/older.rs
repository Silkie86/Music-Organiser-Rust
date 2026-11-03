//! The backup's older versions. When a run replaces a small file on the backup (one you changed since the last
//! backup), the copy it replaces is kept in `<backup>\Older versions\<run>\…`, at the same path below that as it had on
//! the backup ([`crate::copy`]), so a file you overwrote by mistake can still be found. Each run's folder is named
//! after the minute the run began ("2026-11-03 14.40").
//!
//! Solomon, 3 Nov 2026: they're kept for ever, and he can remove them when he wants.
//!
//! - [`read`] lists them, a run at a time. It only reads.
//! - [`remove`] removes the runs you pick, for good: not to the Recycle Bin, as the point is to make room on the backup
//!   drive (and a USB stick has no bin). The app says what goes and asks first. It takes only what [`read`] listed:
//!   each file only while it's still the size and time it was listed with, never through a link, and each folder only
//!   once it's empty. Anything else in Older versions (a folder not named like a run, a file put there since) stays.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use chrono::NaiveDateTime;
use rayon::prelude::*;

use crate::copy::{CantStart, Drive, OLDER_VERSIONS};

/// How a run's folder is named: the minute the run began.
const STAMP: &str = "%Y-%m-%d %H.%M";

/// How deep a run's folder is walked: far deeper than the backup ever places a file. Deeper folders stay.
const DEEPEST: usize = 128;

/// How often (in files) a removal looks to see the backup drive is still there.
const LOOK_EVERY: usize = 64;

/// The name of a run's folder in Older versions: the minute the run began ("2026-11-03 14.40").
pub fn stamp(when: NaiveDateTime) -> String {
    when.format(STAMP).to_string()
}

/// When a run began, from its folder's name; `None` for a name the backup never gives a run's folder.
pub fn stamp_time(name: &str) -> Option<NaiveDateTime> {
    let when = NaiveDateTime::parse_from_str(name, STAMP).ok()?;
    (stamp(when) == name).then_some(when)
}

/// One older version: the copy of a file that a run replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    /// Where it is, inside its run's folder.
    pub path: PathBuf,
    pub size: u64,
    /// Its time when it was listed: it's only removed while it still has it.
    pub modified: Option<SystemTime>,
}

/// One run's older versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// The name of the run's folder in Older versions.
    pub stamp: String,
    /// When the run began.
    pub when: NaiveDateTime,
    pub versions: Vec<Version>,
}

impl Run {
    /// The room its older versions take.
    pub fn bytes(&self) -> u64 {
        self.versions.iter().map(|v| v.size).fold(0, u64::saturating_add)
    }
}

/// What Older versions holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    /// Each run's older versions, the oldest run first. A run's folder with nothing left in it is listed too, so
    /// removing it tidies the folder away.
    pub runs: Vec<Run>,
    /// Things the backup didn't put there (a file or a folder not named like a run's, a link, something that couldn't
    /// be read): they're left alone and never removed.
    pub others: usize,
}

impl Kept {
    /// How many older versions there are.
    pub fn files(&self) -> usize {
        self.runs.iter().map(|r| r.versions.len()).sum()
    }

    /// The room they take.
    pub fn bytes(&self) -> u64 {
        self.runs.iter().map(Run::bytes).fold(0, u64::saturating_add)
    }

    /// The runs that began before `cutoff`, the oldest first; every run for `None`.
    pub fn before(&self, cutoff: Option<NaiveDateTime>) -> Vec<&Run> {
        self.runs.iter().filter(|r| cutoff.is_none_or(|c| r.when < c)).collect()
    }
}

/// Lists the older versions on a backup, a run at a time, each run's folder walked on the current rayon pool (call it
/// inside the app's low-priority pool, never on the window's thread). Only reads; links are never followed. A backup
/// with no Older versions yet gives an empty list; a backup folder that isn't there (the drive isn't plugged in) is
/// an error.
pub fn read(backup: &Path, stop: &AtomicBool) -> io::Result<Kept> {
    if Drive::find(backup).is_none() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "The backup folder isn't there"));
    }
    let older = backup.join(OLDER_VERSIONS);
    match fs::symlink_metadata(&older) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Kept::default()),
        Err(e) => return Err(e),
        // A file or a link where the folder goes: not the backup's own folder, so nothing in it is listed.
        Ok(meta) if !meta.is_dir() => return Ok(Kept { runs: Vec::new(), others: 1 }),
        Ok(_) => {}
    }
    let mut folders: Vec<(String, NaiveDateTime, PathBuf)> = Vec::new();
    let mut others = 0;
    for entry in fs::read_dir(&older)? {
        // The entry's own type: a link to a folder is a link, never the folder it leads to.
        let run = entry.ok().and_then(|e| {
            let name = e.file_name().into_string().ok()?;
            let when = stamp_time(&name)?;
            e.file_type().ok()?.is_dir().then(|| (name, when, e.path()))
        });
        match run {
            Some(run) => folders.push(run),
            None => others += 1,
        }
    }
    let walked: Vec<(Run, usize)> = folders
        .into_par_iter()
        .map(|(stamp, when, folder)| {
            let w = walk(&folder, stop)?;
            Ok((Run { stamp, when, versions: w.files }, w.others))
        })
        .collect::<io::Result<_>>()?;
    let mut kept = Kept { runs: Vec::with_capacity(walked.len()), others };
    for (run, run_others) in walked {
        kept.others += run_others;
        kept.runs.push(run);
    }
    kept.runs.sort_by(|a, b| a.when.cmp(&b.when).then_with(|| a.stamp.cmp(&b.stamp)));
    Ok(kept)
}

/// What a run's folder holds, as a walk that never follows a link finds it.
#[derive(Default)]
struct Walked {
    files: Vec<Version>,
    /// The folders inside it (not the run's folder itself).
    dirs: Vec<PathBuf>,
    /// Links, folders that couldn't be read or were too deep, and anything else that isn't a plain file or folder.
    others: usize,
}

/// Walks a run's folder. A folder that can't be read is left out (it counts as another thing); `stop` stops it with
/// an `Interrupted` error.
fn walk(folder: &Path, stop: &AtomicBool) -> io::Result<Walked> {
    let mut out = Walked::default();
    let mut todo: Vec<(PathBuf, usize)> = vec![(folder.to_path_buf(), 0)];
    while let Some((dir, depth)) = todo.pop() {
        if stop.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Stopped"));
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            out.others += 1;
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                out.others += 1;
                continue;
            };
            // The entry's own type, so a link (a junction on Windows too) is never taken for what it leads to.
            match entry.file_type() {
                Ok(kind) if kind.is_dir() && depth < DEEPEST => {
                    out.dirs.push(entry.path());
                    todo.push((entry.path(), depth + 1));
                }
                // On Windows from the folder listing itself; elsewhere the file's own details, not a link's target.
                Ok(kind) if kind.is_file() => match entry.metadata() {
                    Ok(meta) => {
                        out.files.push(Version { path: entry.path(), size: meta.len(), modified: meta.modified().ok() })
                    }
                    Err(_) => out.others += 1,
                },
                _ => out.others += 1,
            }
        }
    }
    Ok(out)
}

/// What a removal did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removed {
    /// How many older versions went, and the room they took.
    pub files: usize,
    pub bytes: u64,
    /// Listed older versions that stayed, and why: changed since they were listed, a run's folder that isn't the
    /// backup's own any more, or the system wouldn't remove them (open in another program, read-only).
    pub left: Vec<(PathBuf, String)>,
    /// Things in the runs' folders that weren't listed (put there since, links): left alone.
    pub others: usize,
    /// You stopped it part way: the runs after that stay.
    pub stopped: bool,
    /// The backup drive went away part way: the runs after that stay.
    pub gone: bool,
}

/// Removes the older versions of `runs` (as [`read`] listed them), for good, once you've said yes. Each run's folder
/// is walked again and only what was listed goes, each file only while it's still the size and time it was listed
/// with. Links are never followed or removed. Each folder goes once it's empty, the run's own folder last. Anything
/// else stays. Runs go in the order given (oldest first, from [`Kept::before`]), so a stopped removal leaves the
/// newest ones. `progress` hears how many listed versions have been dealt with, of all of them. Call it away from the
/// window's thread.
pub fn remove(
    backup: &Path,
    runs: &[&Run],
    stop: &AtomicBool,
    progress: &dyn Fn(usize, usize),
) -> Result<Removed, CantStart> {
    let Some(drive) = Drive::find(backup) else {
        return Err(CantStart::NoBackupFolder(backup.to_path_buf()));
    };
    let mut out = Removed::default();
    let total: usize = runs.iter().map(|r| r.versions.len()).sum();
    // Listed versions dealt with in the runs before this one.
    let mut before = 0usize;
    let older = backup.join(OLDER_VERSIONS);
    let real_folder = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
    let older_is_own = real_folder(&older);
    for run in runs {
        if stop.load(Ordering::Relaxed) {
            out.stopped = true;
            break;
        }
        if !drive.still_there() {
            out.gone = true;
            break;
        }
        let folder = older.join(&run.stamp);
        // Only the backup's own run folders, straight inside its own Older versions folder.
        if !older_is_own || stamp_time(&run.stamp) != Some(run.when) || !real_folder(&folder) {
            let why = "Its backup's folder in Older versions isn't there any more, or isn't the backup's own";
            out.left.extend(run.versions.iter().map(|v| (v.path.clone(), why.to_owned())));
            before += run.versions.len();
            progress(before, total);
            continue;
        }
        let walked = match walk(&folder, stop) {
            Ok(w) => w,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                out.stopped = true;
                break;
            }
            Err(e) => {
                out.left.extend(run.versions.iter().map(|v| (v.path.clone(), e.to_string())));
                before += run.versions.len();
                progress(before, total);
                continue;
            }
        };
        out.others += walked.others;
        let listed: HashMap<&Path, &Version> = run.versions.iter().map(|v| (v.path.as_path(), v)).collect();
        let mut seen = 0usize;
        for found in &walked.files {
            let Some(listed) = listed.get(found.path.as_path()) else {
                out.others += 1;
                continue;
            };
            if stop.load(Ordering::Relaxed) {
                out.stopped = true;
                break;
            }
            if seen % LOOK_EVERY == LOOK_EVERY - 1 && !drive.still_there() {
                out.gone = true;
                break;
            }
            seen += 1;
            if found.size != listed.size || found.modified.is_none() || found.modified != listed.modified {
                out.left.push((found.path.clone(), "It changed since it was listed".to_owned()));
            } else {
                match fs::remove_file(&found.path) {
                    Ok(()) => {
                        out.files += 1;
                        out.bytes = out.bytes.saturating_add(found.size);
                    }
                    Err(_) if !drive.still_there() => {
                        out.gone = true;
                        break;
                    }
                    Err(e) => out.left.push((found.path.clone(), e.to_string())),
                }
            }
            progress(before + seen.min(run.versions.len()), total);
        }
        before += run.versions.len();
        progress(before, total);
        if out.gone {
            break;
        }
        // Each folder once it's empty, the deepest first, then the run's own. A folder that isn't empty stays.
        let mut dirs = walked.dirs;
        dirs.sort_by_key(|d| Reverse(d.components().count()));
        for d in dirs.iter().chain(std::iter::once(&folder)) {
            if real_folder(d) {
                let _ = fs::remove_dir(d);
            }
        }
        if out.stopped {
            break;
        }
    }
    Ok(out)
}
