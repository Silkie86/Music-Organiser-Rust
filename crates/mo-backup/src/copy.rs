//! Copying the backup list to the backup drive: each file goes in under a temporary name, is read back and checked
//! against what was read from your folder, and only then takes its real name. Your music folders are only ever read.
//!
//! A file already on the backup with the same size and time (within a drive's clock steps) is left as it is. When a
//! file has changed since the last backup, the copy it replaces is kept in `<backup>\Older versions\<stamp>\…` if it's
//! small (Solomon, 31 Oct 2026: big files don't pile up old copies), so a small file you overwrote by mistake can still
//! be found. An old copy that's byte for byte the new one (only its time changed) isn't kept twice.
//!
//! The small file helpers here (the 2-second time window, keeping a time on a read-only file, replacing a read-only
//! file) follow mo-space's folder move, which keeps its own private; they behave the same.

use std::collections::HashSet;
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use rayon::prelude::*;
use sha2::{Digest, Sha256};

use crate::plan::{DEFAULT_BIG_SAVE, place_on_backup};
use mo_space::size::FileEntry;

/// The folder on the backup drive that keeps replaced copies of small files.
pub const OLDER_VERSIONS: &str = "Older versions";

/// What a file being copied is called until it has been checked.
const PART: &str = ".mo-part";

/// Times this close count as the same: FAT and exFAT drives (most USB drives) keep times in 2-second or 10 ms steps.
const SAME_TIME: Duration = Duration::from_secs(2);

/// Files at least this big are copied one at a time: a USB drive copes far better with one big stream than several.
const BIG_COPY: u64 = 64 << 20;

/// The most read in one go.
const BUFFER: usize = 1 << 20;

/// How a copy runs (Settings).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Replaced copies smaller than this are kept in [`OLDER_VERSIONS`]; bigger ones are not.
    pub keep_older_below: u64,
    /// The folder name this run's older versions go under ("2026-10-31 22.40").
    pub stamp: String,
}

impl Options {
    pub fn new(stamp: impl Into<String>) -> Options {
        Options { keep_older_below: DEFAULT_BIG_SAVE, stamp: stamp.into() }
    }
}

/// What happened to one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Copied for the first time.
    New,
    /// Already on the backup (same size and time, or byte for byte the same): nothing copied.
    UpToDate,
    /// Changed since the last backup: copied again; the old copy was kept in [`OLDER_VERSIONS`] when it's small.
    Updated { kept_older: bool },
    /// Not reached: the run was stopped, or the backup drive filled up, first.
    NotReached,
    /// Couldn't be backed up this time (it stays as it was on the backup, if it was there).
    Failed(String),
}

/// One line of a run's report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub source: PathBuf,
    pub outcome: Outcome,
    /// Bytes written to the backup drive for it.
    pub written: u64,
}

/// What a run did. Every file in the list has a line, even when the run stopped part way.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<Line>,
    /// Stopped by you before the end.
    pub stopped: bool,
    /// The backup drive filled up: the files after that weren't reached.
    pub full: bool,
    /// The backup drive went away part way (unplugged): the files after that weren't reached.
    pub gone: bool,
}

/// Bigger than a FAT32 drive can hold in one file: a "drive full" for such a file doesn't mean the drive is full.
const FAT32_LARGEST: u64 = (4 << 30) - 1;

/// Why a run couldn't start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CantStart {
    /// The backup folder isn't there (the drive isn't plugged in, or not mounted): nothing is created in its place.
    NoBackupFolder(PathBuf),
}

/// One file's work: its place in the list, the file, and where it goes (or why it can't go).
type Work<'a> = (usize, &'a FileEntry, Result<PathBuf, &'static str>);

/// Copies every file in `files` (the plan's [`crate::plan::Plan::copy`]) into `backup`, which must already be there.
/// Small files go several at a time on the current rayon pool, then big ones one at a time: call it inside the app's
/// low-priority pool, never on the window's thread. A file that fails doesn't stop the rest; `stop` does (files
/// already copied stay, and a half-copied one is cleared away), and so does a full drive.
pub fn run(files: &[FileEntry], backup: &Path, options: &Options, stop: &AtomicBool) -> Result<Report, CantStart> {
    let Some(drive) = Drive::find(backup) else {
        return Err(CantStart::NoBackupFolder(backup.to_path_buf()));
    };
    // Two files going to one place on the backup (names that differ only in letter case, on a drive that ignores
    // it: Windows' and, at first, a Mac's) would be written over each other: the first goes, the rest say why not.
    let ignores_case = cfg!(any(windows, target_os = "macos"));
    let mut places: HashSet<String> = HashSet::new();
    let work: Vec<Work<'_>> = files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let place = if f.path.to_string_lossy().ends_with(PART) {
                Err("Named like the backup's own half-done copies (.mo-part)")
            } else {
                place_on_backup(backup, &f.path).ok_or("This path can't be placed on the backup")
            };
            let place = place.and_then(|d| {
                let name = d.to_string_lossy();
                let name = if ignores_case { name.to_lowercase() } else { name.into_owned() };
                if places.insert(name) { Ok(d) } else { Err("Another file goes to the same place on the backup") }
            });
            (i, f, place)
        })
        .collect();
    let full = AtomicBool::new(false);
    let gone = AtomicBool::new(false);
    let do_one = |(i, f, place): &Work<'_>| -> (usize, Line) {
        let (outcome, written) =
            if stop.load(Ordering::Relaxed) || full.load(Ordering::Relaxed) || gone.load(Ordering::Relaxed) {
                (Outcome::NotReached, 0)
            } else if !drive.still_there() {
                gone.store(true, Ordering::Relaxed);
                (Outcome::NotReached, 0)
            } else {
                match place {
                    Err(why) => (Outcome::Failed((*why).to_owned()), 0),
                    Ok(dest) => match one(f, dest, backup, options, stop) {
                        Ok(done) => done,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted && stop.load(Ordering::Relaxed) => {
                            (Outcome::NotReached, 0)
                        }
                        Err(e) if e.kind() == io::ErrorKind::StorageFull && f.size > FAT32_LARGEST => {
                            (Outcome::Failed("Too big for this drive: a FAT32 drive holds files up to 4 GB".into()), 0)
                        }
                        Err(e) => {
                            if e.kind() == io::ErrorKind::StorageFull {
                                full.store(true, Ordering::Relaxed);
                            }
                            (Outcome::Failed(e.to_string()), 0)
                        }
                    },
                }
            };
        (*i, Line { source: f.path.clone(), outcome, written })
    };
    let (small, big): (Vec<&Work<'_>>, Vec<&Work<'_>>) = work.iter().partition(|(_, f, _)| f.size < BIG_COPY);
    let mut lines: Vec<(usize, Line)> = small.par_iter().map(|w| do_one(w)).collect();
    lines.extend(big.iter().map(|w| do_one(w)));
    lines.sort_by_key(|(i, _)| *i);
    let missed = lines.iter().any(|(_, l)| l.outcome == Outcome::NotReached);
    let full = full.load(Ordering::Relaxed);
    let gone = gone.load(Ordering::Relaxed);
    Ok(Report {
        lines: lines.into_iter().map(|(_, l)| l).collect(),
        stopped: missed && !full && !gone && stop.load(Ordering::Relaxed),
        full,
        gone,
    })
}

/// The backup folder as it was when the run began, to notice it going away: a drive unplugged, or (on a Mac or Linux)
/// unmounted, which leaves an empty folder on the system disk where it was.
struct Drive<'a> {
    folder: &'a Path,
    #[cfg(unix)]
    device: u64,
}

impl<'a> Drive<'a> {
    fn find(folder: &'a Path) -> Option<Drive<'a>> {
        let meta = fs::metadata(folder).ok().filter(|m| m.is_dir())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(Drive { folder, device: meta.dev() })
        }
        #[cfg(not(unix))]
        {
            let _ = meta;
            Some(Drive { folder })
        }
    }

    fn still_there(&self) -> bool {
        let Ok(meta) = fs::metadata(self.folder) else { return false };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            meta.is_dir() && meta.dev() == self.device
        }
        #[cfg(not(unix))]
        meta.is_dir()
    }
}

fn one(f: &FileEntry, dest: &Path, backup: &Path, options: &Options, stop: &AtomicBool) -> io::Result<(Outcome, u64)> {
    let source_meta = fs::metadata(&f.path)?;
    let old = match fs::symlink_metadata(dest) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        // Anything else (a failing drive, no permission): leave the copy there as it is.
        Err(e) => return Err(e),
    };
    if let Some(old) = &old {
        if !old.is_file() {
            return Err(io::Error::other("Something that isn't a file is in its place on the backup"));
        }
        if same_file(old, &source_meta) {
            return Ok((Outcome::UpToDate, 0));
        }
        // Same size, another time (a clock change shifts every time on a FAT drive; a sync tool touches files):
        // when the bytes are the same, the copy there just takes the new time. Reading both is cheaper than writing.
        if old.len() == source_meta.len() && hash_file(dest, stop)? == hash_file(&f.path, stop)? {
            keep_time(dest, source_meta.modified()?)?;
            return Ok((Outcome::UpToDate, 0));
        }
    }
    let Some(folder) = dest.parent() else { return Err(io::Error::other("No folder on the backup")) };
    fs::create_dir_all(folder)?;
    let part = part_name(dest);
    match fs::symlink_metadata(&part) {
        Ok(m) if !m.is_file() => return Err(io::Error::other("Something that isn't a file is in the way of the copy")),
        // Left by a run that stopped.
        Ok(_) => remove_part(&part),
        Err(_) => {}
    }
    let source = Source { path: &f.path, size: source_meta.len(), time: source_meta.modified()? };
    let done = finish(&source, dest, &part, old.as_ref(), backup, options, stop);
    if done.is_err() {
        remove_part(&part);
    }
    done
}

/// The file in your folder, as it was before the copy began.
struct Source<'a> {
    path: &'a Path,
    size: u64,
    time: SystemTime,
}

/// Everything after the temporary copy's name is free: copy, check, keep the older version, put in place. The caller
/// clears the temporary copy away if this fails.
fn finish(
    source: &Source<'_>,
    dest: &Path,
    part: &Path,
    old: Option<&Metadata>,
    backup: &Path,
    options: &Options,
    stop: &AtomicBool,
) -> io::Result<(Outcome, u64)> {
    let written = copy_checked(source, part, stop)?;
    let Some(old) = old else {
        put_in_place(part, dest)?;
        return Ok((Outcome::New, written));
    };
    let mut kept: Option<PathBuf> = None;
    if old.len() < options.keep_older_below {
        let rel = dest.strip_prefix(backup).map_err(io::Error::other)?;
        let keep = backup.join(OLDER_VERSIONS).join(&options.stamp).join(rel);
        if let Some(p) = keep.parent() {
            fs::create_dir_all(p)?;
        }
        let keep = claim_free_name(&keep)?;
        if let Err(e) = fs::rename(dest, &keep) {
            let _ = fs::remove_file(&keep);
            return Err(e);
        }
        kept = Some(keep);
    }
    if let Err(e) = put_in_place(part, dest) {
        // The old copy goes back where it was.
        if let Some(keep) = &kept {
            let _ = fs::rename(keep, dest);
        }
        return Err(e);
    }
    Ok((Outcome::Updated { kept_older: kept.is_some() }, written))
}

/// `<name>.mo-part` beside the file.
fn part_name(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(PART);
    dest.with_file_name(name)
}

/// Claims the path, or "name (2).ext", "name (3).ext"… when something is there already, by making an empty file there
/// (only one thread or run can make it), which the kept version then replaces. So no kept version is ever replaced.
fn claim_free_name(path: &Path) -> io::Result<PathBuf> {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for n in 1u32..10_000 {
        let candidate = if n == 1 { path.to_path_buf() } else { path.with_file_name(format!("{stem} ({n}){ext}")) };
        match File::create_new(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("Too many older versions of this file in one run"))
}

/// Copies the source to `to`, reading it once, then reads `to` back and checks both read the same (after the copy is
/// flushed to the drive; the read-back may still come from the system's memory cache, so a "Check backup" re-reads
/// later). Also checks the source wasn't saved again while it was copied (its size and time), and gives `to` the
/// source's time, so the next backup sees it as up to date. Gives the bytes written.
fn copy_checked(source: &Source<'_>, to: &Path, stop: &AtomicBool) -> io::Result<u64> {
    let mut src = File::open(source.path)?;
    let mut out = File::create(to)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; buffer_for(source.size)];
    let mut total: u64 = 0;
    while let Some(n) = read_some(&mut src, &mut buf, stop)? {
        hash.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        total = total.saturating_add(n as u64);
    }
    out.sync_all()?;
    drop(out);

    let after = fs::metadata(source.path)?;
    if after.len() != total || total != source.size || after.modified()? != source.time {
        return Err(io::Error::other("It changed while it was being copied (saved again?): it goes next time"));
    }
    let wrote: [u8; 32] = hash.finalize().into();
    if hash_file(to, stop)? != wrote {
        return Err(io::Error::other("The copy read back differently: the backup drive may be failing"));
    }
    keep_time(to, source.time)?;
    Ok(total)
}

/// A file's SHA-256, read once.
fn hash_file(path: &Path, stop: &AtomicBool) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let size = file.metadata().map_or(BUFFER as u64, |m| m.len());
    let mut buf = vec![0u8; buffer_for(size)];
    let mut hash = Sha256::new();
    while let Some(n) = read_some(&mut file, &mut buf, stop)? {
        hash.update(&buf[..n]);
    }
    Ok(hash.finalize().into())
}

/// A buffer as big as the file, up to [`BUFFER`]: a small preset doesn't need a whole megabyte.
fn buffer_for(size: u64) -> usize {
    usize::try_from(size).unwrap_or(BUFFER).clamp(1, BUFFER)
}

/// Reads the next piece: `None` at the end. Tries again when the system interrupts a read; gives up with
/// `Interrupted` when you stop the run.
fn read_some(file: &mut File, buf: &mut [u8], stop: &AtomicBool) -> io::Result<Option<usize>> {
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Stopped"));
        }
        match file.read(buf) {
            Ok(0) => return Ok(None),
            Ok(n) => return Ok(Some(n)),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// Whether two files look the same: the same size, and times no further apart than a drive's clock steps.
fn same_file(a: &Metadata, b: &Metadata) -> bool {
    a.len() == b.len()
        && match (a.modified(), b.modified()) {
            (Ok(x), Ok(y)) => x.duration_since(y).unwrap_or_else(|e| e.duration()) < SAME_TIME,
            _ => false,
        }
}

/// Puts a checked copy in place of the file there, a read-only one too (Windows won't replace those otherwise).
fn put_in_place(part: &Path, dest: &Path) -> io::Result<()> {
    let was_read_only = set_read_only(dest, false);
    fs::rename(part, dest).inspect_err(|_| {
        if was_read_only {
            set_read_only(dest, true);
        }
    })
}

/// Removes a temporary copy, a read-only one too.
fn remove_part(part: &Path) {
    set_read_only(part, false);
    let _ = fs::remove_file(part);
}

/// Windows only (elsewhere a read-only file can be replaced or removed anyway): makes a file read-only or not, and
/// says whether it changed.
fn set_read_only(path: &Path, read_only: bool) -> bool {
    #[cfg(windows)]
    {
        let Ok(meta) = fs::symlink_metadata(path) else { return false };
        if !meta.is_file() || meta.permissions().readonly() == read_only {
            return false;
        }
        let mut permissions = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(read_only);
        fs::set_permissions(path, permissions).is_ok()
    }
    #[cfg(not(windows))]
    {
        let _ = (path, read_only);
        false
    }
}

/// Gives a copied file the original's time. Opened only to change its times, so a read-only file (a library's own
/// samples often are) works too.
fn keep_time(copy: &Path, when: SystemTime) -> io::Result<()> {
    let mut options = File::options();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        /// Windows' FILE_WRITE_ATTRIBUTES: enough to change a file's times, even a read-only one's.
        const WRITE_ATTRIBUTES: u32 = 0x100;
        options.access_mode(WRITE_ATTRIBUTES);
    }
    #[cfg(not(windows))]
    options.read(true);
    options.open(copy)?.set_modified(when)
}
