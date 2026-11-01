//! Copying the backup list to the backup drive: each file goes in under a temporary name, is read back and checked
//! against what was read from your folder, and only then takes its real name. Your music folders are only ever read.
//!
//! A file is left as it is when the backup's [record](crate::record) says both it and its copy are unchanged since it
//! was copied, or when the copy has the same size and time (within a drive's clock steps), or the same bytes. A new
//! file whose bytes are already on the backup shares them (a hard link: NTFS and most Mac and Linux drives) instead of
//! taking room again; on a drive that can't (exFAT, FAT32) it's copied as usual. When a
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
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use rayon::prelude::*;
use sha2::{Digest, Sha256};

use crate::plan::{DEFAULT_BIG_SAVE, place_on_backup};
use crate::record::{self, Entry, Record};
use mo_space::size::FileEntry;

/// The folder on the backup drive that keeps replaced copies of small files.
pub const OLDER_VERSIONS: &str = "Older versions";

/// What a file being copied is called until it has been checked.
const PART: &str = ".mo-part";

/// Times this close count as the same: FAT and exFAT drives (most USB drives) keep times in 2-second or 10 ms steps.
const SAME_TIME: Duration = Duration::from_secs(2);

/// Files at least this big are copied one at a time: a USB drive copes far better with one big stream than several.
const BIG_COPY: u64 = 64 << 20;

/// Files smaller than this are copied even when the same bytes are on the backup: sharing them saves almost nothing.
const SHARE_FROM: u64 = 64 << 10;

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
    /// Its bytes were already on the backup (another file with the same contents), so it shares them instead of taking
    /// room again (NTFS and most Mac and Linux drives; on exFAT or FAT32 it's copied as usual).
    pub shared: bool,
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
    /// The backup's record of what it holds couldn't be written (the next run reads the files again instead).
    pub record_not_saved: Option<String>,
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
    // A record that can't be read is left alone: this run works without one and doesn't write over it.
    let (record, record_trusted) = match Record::load(backup, ignores_case) {
        Ok(r) => (r, true),
        Err(_) => (Record::default(), false),
    };
    let record = Mutex::new(record);
    let cant_share = AtomicBool::new(false);
    let ctx = Ctx { backup, options, stop, record: &record, record_trusted, ignores_case, cant_share: &cant_share };
    let do_one = |(i, f, place): &Work<'_>| -> (usize, Line) {
        let (outcome, written, shared) =
            if stop.load(Ordering::Relaxed) || full.load(Ordering::Relaxed) || gone.load(Ordering::Relaxed) {
                (Outcome::NotReached, 0, false)
            } else if !drive.still_there() {
                gone.store(true, Ordering::Relaxed);
                (Outcome::NotReached, 0, false)
            } else {
                match place {
                    Err(why) => (Outcome::Failed((*why).to_owned()), 0, false),
                    Ok(dest) => match one(f, dest, &ctx) {
                        Ok(done) => (done.outcome, done.written, done.shared),
                        Err(e) if e.kind() == io::ErrorKind::Interrupted && stop.load(Ordering::Relaxed) => {
                            (Outcome::NotReached, 0, false)
                        }
                        Err(e) if e.kind() == io::ErrorKind::StorageFull && f.size > FAT32_LARGEST => (
                            Outcome::Failed("Too big for this drive: a FAT32 drive holds files up to 4 GB".into()),
                            0,
                            false,
                        ),
                        Err(e) => {
                            if e.kind() == io::ErrorKind::StorageFull {
                                full.store(true, Ordering::Relaxed);
                            }
                            (Outcome::Failed(e.to_string()), 0, false)
                        }
                    },
                }
            };
        (*i, Line { source: f.path.clone(), outcome, written, shared })
    };
    let (small, big): (Vec<&Work<'_>>, Vec<&Work<'_>>) = work.iter().partition(|(_, f, _)| f.size < BIG_COPY);
    let mut lines: Vec<(usize, Line)> = small.par_iter().map(|w| do_one(w)).collect();
    lines.extend(big.iter().map(|w| do_one(w)));
    lines.sort_by_key(|(i, _)| *i);
    let missed = lines.iter().any(|(_, l)| l.outcome == Outcome::NotReached);
    let full = full.load(Ordering::Relaxed);
    let gone = gone.load(Ordering::Relaxed);
    // A record written to a drive that went away would land on the system disk.
    let record = record.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    let record_not_saved = if gone || !drive.still_there() {
        Some("The backup drive went away".to_owned())
    } else if !record_trusted {
        Some("The backup's record couldn't be read, so it was left as it was".to_owned())
    } else {
        record.save(backup).err().map(|e| e.to_string())
    };
    Ok(Report {
        lines: lines.into_iter().map(|(_, l)| l).collect(),
        stopped: missed && !full && !gone && stop.load(Ordering::Relaxed),
        full,
        gone,
        record_not_saved,
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

/// What every file of a run shares.
struct Ctx<'a> {
    backup: &'a Path,
    options: &'a Options,
    stop: &'a AtomicBool,
    record: &'a Mutex<Record>,
    /// The record was read (or there wasn't one yet), so what's learnt now is kept.
    record_trusted: bool,
    ignores_case: bool,
    /// The backup drive turned out not to share bytes between files (exFAT, FAT32): don't try again this run.
    cant_share: &'a AtomicBool,
}

impl Ctx<'_> {
    fn record(&self) -> std::sync::MutexGuard<'_, Record> {
        self.record.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Records what's now at `dest`: the source as it was, the copy's own time, and its bytes' hash. Recording is a
    /// help, not part of the backup: a file that can't be recorded is still backed up, only read again next time.
    fn note(&self, place: Option<&Place>, source: &Source<'_>, dest: &Path, hash: [u8; 32]) {
        let Some(place) = place else { return };
        let Ok(copy_time) = fs::metadata(dest).and_then(|m| m.modified()) else { return };
        let entry = Entry {
            path: place.rel.clone(),
            size: source.size,
            time: record::nanos(source.time),
            copy_time: record::nanos(copy_time),
            hash,
        };
        self.record().set(place.key.clone(), entry);
    }
}

/// Where a file is on the backup, as the record names it.
struct Place {
    rel: String,
    key: String,
}

/// What happened to one file that didn't fail.
struct Done {
    outcome: Outcome,
    written: u64,
    shared: bool,
}

impl Done {
    fn up_to_date() -> Done {
        Done { outcome: Outcome::UpToDate, written: 0, shared: false }
    }
}

fn one(f: &FileEntry, dest: &Path, ctx: &Ctx<'_>) -> io::Result<Done> {
    let source_meta = fs::metadata(&f.path)?;
    let source = Source { path: &f.path, size: source_meta.len(), time: source_meta.modified()? };
    // A name the record can't hold (a backslash in a Mac or Linux name) is backed up without a record line.
    let place = record::rel_path(ctx.backup, dest).map(|rel| Place { key: record::fold(&rel, ctx.ignores_case), rel });
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
        let old_time = record::nanos(old.modified()?);
        let recorded = place.as_ref().and_then(|p| ctx.record().get(&p.key).cloned());
        // Both the file and its copy are as the record says (a shared copy can't carry its own file's time).
        if let Some(e) = &recorded
            && e.size == source.size
            && e.time == record::nanos(source.time)
            && old.len() == source.size
            && e.copy_time == old_time
        {
            return Ok(Done::up_to_date());
        }
        if same_file(old, &source_meta) {
            // Up to date by its time; one not yet recorded (a lost record) is read once so it can be shared. Not when
            // the record can't be saved (it would be read again every run) or the file is too small to share.
            if ctx.record_trusted
                && place.is_some()
                && source.size >= SHARE_FROM
                && recorded.is_none_or(|e| e.size != source.size || e.copy_time != old_time)
            {
                let hash = hash_file(dest, ctx.stop)?;
                ctx.note(place.as_ref(), &source, dest, hash);
            }
            return Ok(Done::up_to_date());
        }
        // Same size, another time (a clock change shifts every time on a FAT drive; a sync tool touches files):
        // when the bytes are the same, the copy there is right. Reading both is cheaper than writing. Its time is
        // left alone (it may be shared with another file); the record says it's up to date.
        if old.len() == source.size {
            let there = hash_file(dest, ctx.stop)?;
            if there == hash_file(&f.path, ctx.stop)? {
                // A copy with a name of its own takes the new time, so the next run sees it by its time; a shared
                // one keeps its time (changing it would change the other file's), and the record says it's up to
                // date.
                if own_copy(old, ctx) {
                    keep_time(dest, source.time)?;
                }
                ctx.note(place.as_ref(), &source, dest, there);
                return Ok(Done::up_to_date());
            }
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
    let done = finish(&source, dest, &part, old.as_ref(), place.as_ref(), ctx);
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

/// Everything after the temporary copy's name is free: share or copy, check, keep the older version, put in place.
/// The caller clears the temporary copy away if this fails.
fn finish(
    source: &Source<'_>,
    dest: &Path,
    part: &Path,
    old: Option<&Metadata>,
    place: Option<&Place>,
    ctx: &Ctx<'_>,
) -> io::Result<Done> {
    let shared = match place {
        Some(p) => share(source, part, &p.key, ctx)?,
        None => None,
    };
    let (written, hash, shared) = match shared {
        Some(hash) => (0, hash, true),
        None => {
            let (written, hash) = copy_checked(source, part, ctx.stop)?;
            (written, hash, false)
        }
    };
    let Some(old) = old else {
        put_in_place(part, dest)?;
        ctx.note(place, source, dest, hash);
        return Ok(Done { outcome: Outcome::New, written, shared });
    };
    let mut kept: Option<PathBuf> = None;
    if old.len() < ctx.options.keep_older_below {
        let rel_path = dest.strip_prefix(ctx.backup).map_err(io::Error::other)?;
        let keep = ctx.backup.join(OLDER_VERSIONS).join(&ctx.options.stamp).join(rel_path);
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
    ctx.note(place, source, dest, hash);
    Ok(Done { outcome: Outcome::Updated { kept_older: kept.is_some() }, written, shared })
}

/// The most places of the same size compared before giving up and copying.
const SHARE_TRIES: usize = 8;

/// When the backup already holds these bytes, makes `part` a second name for them instead of copying: no room taken.
/// The record only says where files of this size are; each is compared with the source byte for byte (stopping at the
/// first difference, so a different file costs little), and after sharing, what `part` now holds is read back and
/// checked. Gives the bytes' hash, or `None` when they have to be copied (nothing the same, or a drive that can't share,
/// like exFAT).
fn share(source: &Source<'_>, part: &Path, key: &str, ctx: &Ctx<'_>) -> io::Result<Option<[u8; 32]>> {
    // Tiny files aren't worth comparing.
    if source.size < SHARE_FROM || ctx.cant_share.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let candidates = ctx.record().same_size(source.size, key, SHARE_TRIES);
    for rel in candidates {
        let Some(there) = record::place(ctx.backup, &rel) else { continue };
        // A place that can't be read now (moved to Older versions by another file's update, a bad sector) is just
        // not used.
        let Ok(Some((hash, seen))) = same_bytes(source.path, &there, ctx.stop) else {
            if ctx.stop.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "Stopped"));
            }
            continue;
        };
        if let Err(e) = fs::hard_link(&there, part) {
            if cant_link_here(&e) {
                // This drive can't share: copy as usual, and don't try again this run.
                ctx.cant_share.store(true, Ordering::Relaxed);
                return Ok(None);
            }
            // Too many names already, or the file moved just now: try the next place.
            continue;
        }
        // `part` must be the very file that was compared (another file's update may have put a new one at `there`
        // in between): same size and time, and on a Mac or Linux the same file number. And the source mustn't have
        // changed while it was read.
        let after = fs::metadata(source.path)?;
        let linked = fs::metadata(part)?;
        if !seen.is(&linked) || after.len() != source.size || after.modified()? != source.time {
            remove_part(part);
            continue;
        }
        return Ok(Some(hash));
    }
    Ok(None)
}

/// A file as it was when it was read, to tell later whether a name still leads to it.
struct Seen {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    id: (u64, u64),
}

impl Seen {
    fn of(meta: &Metadata) -> Seen {
        Seen {
            len: meta.len(),
            modified: meta.modified().ok(),
            #[cfg(unix)]
            id: {
                use std::os::unix::fs::MetadataExt;
                (meta.dev(), meta.ino())
            },
        }
    }

    fn is(&self, meta: &Metadata) -> bool {
        let other = Seen::of(meta);
        #[cfg(unix)]
        if self.id != other.id {
            return false;
        }
        self.len == other.len && self.modified.is_some() && self.modified == other.modified
    }
}

/// Whether linking failed because this drive can't share files at all (exFAT, FAT32, some network drives), rather
/// than because of the one file.
fn cant_link_here(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::Unsupported || e.kind() == io::ErrorKind::CrossesDevices {
        return true;
    }
    match e.raw_os_error() {
        // EPERM, EXDEV, ENOTSUP/EOPNOTSUPP (Linux 95, Mac 45 and 102).
        Some(code) if cfg!(unix) => matches!(code, 1 | 18 | 95 | 45 | 102),
        // ERROR_INVALID_FUNCTION, ERROR_ACCESS_DENIED (FAT), ERROR_NOT_SAME_DEVICE, ERROR_NOT_SUPPORTED.
        Some(code) if cfg!(windows) => matches!(code, 1 | 5 | 17 | 50),
        _ => false,
    }
}

/// Whether a copy on the backup has a name of its own (changing its time changes no other file). Mac and Linux can
/// tell; on Windows only a drive that turned out not to share can be sure.
fn own_copy(meta: &Metadata, ctx: &Ctx<'_>) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = ctx;
        meta.nlink() == 1
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        ctx.cant_share.load(Ordering::Relaxed)
    }
}

/// Compares two files byte for byte, stopping at the first difference. Gives the SHA-256 of their bytes when they're
/// the same (and how `b` looked when opened), `None` when they differ.
fn same_bytes(a: &Path, b: &Path, stop: &AtomicBool) -> io::Result<Option<([u8; 32], Seen)>> {
    let mut fa = File::open(a)?;
    let mut fb = File::open(b)?;
    let size = fa.metadata()?.len();
    let b_meta = fb.metadata()?;
    if b_meta.len() != size {
        return Ok(None);
    }
    let seen = Seen::of(&b_meta);
    let mut ba = vec![0u8; buffer_for(size)];
    let mut bb = vec![0u8; buffer_for(size)];
    let mut hash = Sha256::new();
    loop {
        let Some(n) = read_some(&mut fa, &mut ba, stop)? else {
            return Ok(read_some(&mut fb, &mut bb, stop)?.is_none().then(|| (hash.finalize().into(), seen)));
        };
        // Fill the same length from the other file.
        let mut got = 0;
        while got < n {
            match read_some(&mut fb, &mut bb[got..n], stop)? {
                Some(m) => got += m,
                None => return Ok(None),
            }
        }
        if ba[..n] != bb[..n] {
            return Ok(None);
        }
        hash.update(&ba[..n]);
    }
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
/// source's time, so the next backup sees it as up to date. Gives the bytes written and their SHA-256.
fn copy_checked(source: &Source<'_>, to: &Path, stop: &AtomicBool) -> io::Result<(u64, [u8; 32])> {
    let mut src = File::open(source.path)?;
    // A new file only: should `to` still be there (a temporary copy that couldn't be cleared away, perhaps a second
    // name of another file on the backup), writing into it could change that file.
    let mut out = File::create_new(to)?;
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
    Ok((total, wrote))
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
