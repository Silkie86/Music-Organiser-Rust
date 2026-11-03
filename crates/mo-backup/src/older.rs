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
//!   each file only while it's still the size and time it was listed with, never through a link, and the folders it
//!   empties. Anything else in Older versions (a folder not named like a run, a file put there since) stays.
//!
//! The backup shares the bytes of files that are the same (one file with two names, [`crate::copy`]), so an older
//! version can share its bytes with another file on the backup. Removing it frees no room until every name goes:
//! [`freed`] counts the room each removal really gives back.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use chrono::NaiveDateTime;

use crate::copy::{CantStart, Drive, OLDER_VERSIONS, set_read_only};

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

/// A file with more than one name on its drive (the backup's sharing): removing one name frees nothing until every
/// name goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shared {
    /// Which file it is on its drive.
    pub id: (u64, u64),
    /// How many names it has, in Older versions and anywhere else on the drive.
    pub names: u64,
}

/// One older version: the copy of a file that a run replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    /// Where it is, inside its run's folder.
    pub path: PathBuf,
    pub size: u64,
    /// Its time when it was listed: it's only removed while it still has it.
    pub modified: Option<SystemTime>,
    /// When the file has other names too; `None` for one with just this name (or when the system can't say, as on a
    /// drive that can't share).
    pub shared: Option<Shared>,
}

/// The room removing these older versions gives back: each one's size, but a file with more than one name only once,
/// and only when every one of its names is among them.
pub fn freed<'a>(versions: impl IntoIterator<Item = &'a Version>) -> u64 {
    let mut bytes: u64 = 0;
    // Each shared file: its names, how many of them are here, and its size.
    let mut shared: HashMap<(u64, u64), (u64, u64, u64)> = HashMap::new();
    for v in versions {
        match v.shared {
            None => bytes = bytes.saturating_add(v.size),
            Some(s) => shared.entry(s.id).or_insert((s.names, 0, v.size)).1 += 1,
        }
    }
    for (names, here, size) in shared.into_values() {
        if here >= names {
            bytes = bytes.saturating_add(size);
        }
    }
    bytes
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
    /// The room removing just this run's older versions gives back.
    pub fn bytes(&self) -> u64 {
        freed(&self.versions)
    }
}

/// What Older versions holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    /// Each run's older versions, the oldest run first. A run's folder with nothing left in it is listed too, so a
    /// removal that takes its time takes it along.
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

    /// The room they take: what removing all of them gives back.
    pub fn bytes(&self) -> u64 {
        freed(self.runs.iter().flat_map(|r| &r.versions))
    }

    /// The runs that began before `cutoff`, the oldest first; every run for `None`.
    pub fn before(&self, cutoff: Option<NaiveDateTime>) -> Vec<&Run> {
        self.runs.iter().filter(|r| cutoff.is_none_or(|c| r.when < c)).collect()
    }
}

/// Lists the older versions on a backup, one run's folder after another (one drive, so no faster side by side). Call
/// it away from the window's thread. Only reads; links are never followed. A backup with no Older versions yet gives
/// an empty list; a backup folder that isn't there (the drive isn't plugged in) is an error.
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
    let mut kept = Kept::default();
    for entry in fs::read_dir(&older)? {
        // The entry's own type: a link to a folder is a link, never the folder it leads to.
        let run = entry.ok().and_then(|e| {
            let name = e.file_name().into_string().ok()?;
            let when = stamp_time(&name)?;
            e.file_type().ok()?.is_dir().then(|| (name, when, e.path()))
        });
        let Some((stamp, when, folder)) = run else {
            kept.others += 1;
            continue;
        };
        match walk(&folder, true, stop) {
            Ok(w) => {
                kept.others += w.others;
                kept.runs.push(Run { stamp, when, versions: w.files });
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
            // A run's folder that can't be read can't be listed, so nothing in it is removed.
            Err(_) => kept.others += 1,
        }
    }
    kept.runs.sort_by(|a, b| a.when.cmp(&b.when).then_with(|| a.stamp.cmp(&b.stamp)));
    Ok(kept)
}

/// What a run's folder holds, as a walk that never follows a link finds it.
#[derive(Default)]
struct Walked {
    files: Vec<Version>,
    /// Links, folders inside that couldn't be read or were too deep, and anything else that isn't a plain file or
    /// folder.
    others: usize,
}

/// Walks a run's folder; `shared` also asks which files have other names. The run's folder itself not being readable
/// is an error; a folder inside it that can't be read is left out (it counts as another thing). `stop` stops it with
/// an `Interrupted` error.
fn walk(folder: &Path, shared: bool, stop: &AtomicBool) -> io::Result<Walked> {
    let mut out = Walked::default();
    let mut todo: Vec<(PathBuf, usize)> = vec![(folder.to_path_buf(), 0)];
    while let Some((dir, depth)) = todo.pop() {
        if stop.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Stopped"));
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if depth == 0 => return Err(e),
            Err(_) => {
                out.others += 1;
                continue;
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                out.others += 1;
                continue;
            };
            // The entry's own type, so a link (a junction on Windows too) is never taken for what it leads to.
            match entry.file_type() {
                Ok(kind) if kind.is_dir() && depth < DEEPEST => todo.push((entry.path(), depth + 1)),
                // On Windows from the folder listing itself; elsewhere the file's own details, not a link's target.
                Ok(kind) if kind.is_file() => match entry.metadata() {
                    Ok(meta) => {
                        let path = entry.path();
                        let shared = if shared { names(&path, &meta) } else { None };
                        out.files.push(Version { path, size: meta.len(), modified: meta.modified().ok(), shared });
                    }
                    Err(_) => out.others += 1,
                },
                _ => out.others += 1,
            }
        }
    }
    Ok(out)
}

/// Which file this is and how many names it has, when it has more than one.
#[cfg(unix)]
fn names(_path: &Path, meta: &Metadata) -> Option<Shared> {
    use std::os::unix::fs::MetadataExt;
    (meta.nlink() > 1).then(|| Shared { id: (meta.dev(), meta.ino()), names: meta.nlink() })
}

/// Which file this is and how many names it has, when it has more than one. Windows only says when the file is
/// opened: opened just to ask, not to read, and not through a link.
#[cfg(windows)]
fn names(path: &Path, _meta: &Metadata) -> Option<Shared> {
    use std::os::windows::fs::OpenOptionsExt;
    /// FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT.
    const JUST_THIS: u32 = 0x0200_0000 | 0x0020_0000;
    let file = fs::OpenOptions::new().access_mode(0).custom_flags(JUST_THIS).open(path).ok()?;
    let info = winapi_util::file::information(&file).ok()?;
    let names = info.number_of_links();
    (names > 1).then(|| Shared { id: (info.volume_serial_number(), info.file_index()), names })
}

#[cfg(not(any(unix, windows)))]
fn names(_path: &Path, _meta: &Metadata) -> Option<Shared> {
    None
}

/// Why an older version picked for removal stayed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// It changed (or something else took its place) since it was listed.
    Changed,
    /// Its folder isn't as it was when listed: gone, a link now, or it couldn't be looked at.
    Folder,
    /// The system wouldn't remove it (open in another program, say): what it said.
    Refused(String),
}

/// What a removal did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removed {
    /// How many older versions went.
    pub files: usize,
    /// The room that gave back ([`freed`]).
    pub bytes: u64,
    /// Listed older versions that stayed, and why.
    pub left: Vec<(PathBuf, Why)>,
    /// Things in the runs' folders that weren't listed (put there since, links): left alone.
    pub others: usize,
    /// You stopped it part way: the runs after that stay.
    pub stopped: bool,
    /// The backup drive went away part way: the runs after that stay.
    pub gone: bool,
}

/// Removes the older versions of `runs` (as [`read`] listed them), for good, once you've said yes. Each run's folder
/// is walked again and only what was listed goes. Just before each file goes, it's checked again: still a plain file of
/// the size and time it was listed with, in real folders all the way down from its run's folder (never through a
/// link). A read-only one goes too. The folders it empties go, the run's own folder last; anything else stays. Runs go
/// in the order given (oldest first, from [`Kept::before`]), so a stopped removal leaves the newest ones. `progress`
/// hears how many listed versions have been dealt with, of all of them. Call it away from the window's thread.
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
    let mut gone_versions: Vec<&Version> = Vec::new();
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
        let leave_all = |out: &mut Removed| out.left.extend(run.versions.iter().map(|v| (v.path.clone(), Why::Folder)));
        // Only the backup's own run folders, straight inside its own Older versions folder.
        if !older_is_own || stamp_time(&run.stamp) != Some(run.when) || !real_folder(&folder) {
            leave_all(&mut out);
            before += run.versions.len();
            progress(before, total);
            continue;
        }
        let walked = match walk(&folder, false, stop) {
            Ok(w) => w,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                out.stopped = true;
                break;
            }
            Err(_) => {
                leave_all(&mut out);
                before += run.versions.len();
                progress(before, total);
                continue;
            }
        };
        out.others += walked.others;
        let by_path: HashMap<&Path, &Version> = run.versions.iter().map(|v| (v.path.as_path(), v)).collect();
        // The folders checked to be real ones, just before the first file in each goes.
        let mut checked: HashMap<PathBuf, bool> = HashMap::new();
        // The folders a file went from, to tidy away once they're empty.
        let mut emptied: HashSet<PathBuf> = HashSet::new();
        let mut seen: HashSet<&Path> = HashSet::new();
        for found in &walked.files {
            let Some(&listed) = by_path.get(found.path.as_path()) else {
                out.others += 1;
                continue;
            };
            if stop.load(Ordering::Relaxed) {
                out.stopped = true;
                break;
            }
            if seen.len() % LOOK_EVERY == LOOK_EVERY - 1 && !drive.still_there() {
                out.gone = true;
                break;
            }
            seen.insert(listed.path.as_path());
            let path = listed.path.as_path();
            let parent = path.parent().unwrap_or(&folder);
            if !real_down(&folder, parent, &mut checked) {
                out.left.push((path.to_path_buf(), Why::Folder));
            } else if !as_listed(path, listed) {
                out.left.push((path.to_path_buf(), Why::Changed));
            } else {
                match remove_version(path, listed.shared.is_some()) {
                    Ok(()) => {
                        out.files += 1;
                        gone_versions.push(listed);
                        emptied.insert(parent.to_path_buf());
                    }
                    Err(_) if !drive.still_there() => {
                        out.gone = true;
                        break;
                    }
                    Err(e) => out.left.push((path.to_path_buf(), Why::Refused(e.to_string()))),
                }
            }
            progress(before + seen.len(), total);
        }
        if !out.stopped && !out.gone {
            // Listed but not found again as a file: gone already (nothing to do), something else in its place now, or
            // in a folder that couldn't be looked at.
            for v in run.versions.iter().filter(|v| !seen.contains(v.path.as_path())) {
                match fs::symlink_metadata(&v.path) {
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Ok(_) => out.left.push((v.path.clone(), Why::Changed)),
                    Err(_) => out.left.push((v.path.clone(), Why::Folder)),
                }
            }
        }
        before += run.versions.len();
        progress(before, total);
        if out.gone {
            break;
        }
        tidy(&folder, emptied);
        if out.stopped {
            break;
        }
    }
    out.bytes = freed(gone_versions);
    Ok(out)
}

/// Whether every folder from `top` down to `dir` is a real folder (not a link), each asked about once.
fn real_down(top: &Path, dir: &Path, checked: &mut HashMap<PathBuf, bool>) -> bool {
    if let Some(&ok) = checked.get(dir) {
        return ok;
    }
    let ok = fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir())
        && (dir == top || dir.parent().is_some_and(|up| up.starts_with(top) && real_down(top, up, checked)));
    checked.insert(dir.to_path_buf(), ok);
    ok
}

/// Whether an older version is still as it was listed, looked at again just now: a plain file (not a link) of the same
/// size and time.
fn as_listed(path: &Path, listed: &Version) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| {
        m.is_file() && m.len() == listed.size && listed.modified.is_some() && m.modified().ok() == listed.modified
    })
}

/// Removes one older version, a read-only one too. On Windows, Rust removes a read-only file by itself where the drive
/// allows it (NTFS), leaving the read-only mark alone: the mark belongs to the file, not the name, so a file with other
/// names (`shared`) keeps it for them. On a FAT or exFAT stick (where no file has two names) that fails: there the mark
/// is cleared and it's tried again, and the mark goes back if the file still can't be removed.
fn remove_version(path: &Path, shared: bool) -> io::Result<()> {
    let first = match fs::remove_file(path) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    if shared || first.kind() != io::ErrorKind::PermissionDenied || !set_read_only(path, false) {
        return Err(first);
    }
    fs::remove_file(path).inspect_err(|_| {
        set_read_only(path, true);
    })
}

/// Removes the folders files went from once they're empty, and the folders above them up to the run's own (which goes
/// once it's empty, as a run with nothing left does), the deepest first. A folder that isn't empty stays, and so does
/// one no file went from (a folder of yours, say).
fn tidy(run_folder: &Path, emptied: HashSet<PathBuf>) {
    let mut dirs: HashSet<PathBuf> = HashSet::from([run_folder.to_path_buf()]);
    for dir in emptied {
        for d in dir.ancestors() {
            if !d.starts_with(run_folder) || !dirs.insert(d.to_path_buf()) {
                break;
            }
        }
    }
    let mut dirs: Vec<PathBuf> = dirs.into_iter().collect();
    dirs.sort_by_key(|d| Reverse(d.components().count()));
    for d in &dirs {
        if fs::symlink_metadata(d).is_ok_and(|m| m.is_dir()) {
            let _ = fs::remove_dir(d);
        }
    }
}

impl std::fmt::Display for Why {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Why::Changed => f.write_str("It changed since it was listed"),
            Why::Folder => {
                f.write_str("Its folder isn't as it was when listed (gone, or a link now), or couldn't be read")
            }
            Why::Refused(e) => write!(f, "The system wouldn't remove it: {e}"),
        }
    }
}
