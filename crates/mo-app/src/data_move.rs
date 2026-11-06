//! Moving the app's data folder to another drive (Solomon, 2026-11-05: C: was 98.8% full and slow, so the folder goes
//! to the fast I: drive). Copy, check, then switch, and the old folder stays until he removes it:
//!
//! 1. **Copy** ([`copy`], from Settings, in the background while the app runs): every file of the data folder goes to
//!    the new one, read once, written as `<name>.mo-move`, read back and compared by SHA-256, given the original's
//!    time (so every saved copy and stamp that goes by size and time stays good), then named. A file that changed
//!    while it was copied, or that a running part of the app holds locked (this run's own log), is copied again in a
//!    second pass, and what still changes waits for step 3. What's done is listed in the new folder's
//!    [`MOVING_IN_FILE`], so a stopped copy goes on where it was.
//! 2. **Ready**: once everything copied and checked, [`PENDING_FILE`] beside the first data folder names the new one.
//!    Nothing switches while the app runs, so every open database stays where it was opened.
//! 3. **Switch** ([`finish_at_start`], at the next start, before any file of the app is opened): one last pass copies
//!    what changed since (normally the last run's log and settings), the same checked way. Only when every file is
//!    there and checked does the new folder get [`paths::MOVED_IN_FILE`] and [`paths::POINTER_FILE`] name it. The old
//!    folder gets a note, [`MOVED_OUT_FILE`], and nothing in it is removed or changed.
//!
//! Left out of the copy: the launchers' own folders ([`LAUNCHER_FOLDERS`]: the launcher runs the app from there and
//! stays where it is), what [`LEFT_BEHIND`] names, the app's own lock files ([`RUNNING_LOCKS`], only for a running app; other `.lock` files, such as a
//! `Cargo.lock` in a tool's folder, are copied) and half-written
//! saves (`*.mo-part`). A database's `-wal` and `-shm` files go with it, so the stamps the app keeps of its databases
//! (which read them) still match after the move. A link or a junction in the folder (`logs` moved to another drive,
//! say) is copied through: the new folder gets a plain folder with its files, so nothing the app reads goes missing; one
//! that leads back into the data folder or the copy is left out. The copy always ends up holding exactly what the data
//! folder holds: a file gone from it since an earlier pass is taken out of the copy too, and so is a half-made one.
//!
//! The switch at start runs before the window opens, so it only takes on a little ([`START_LIMIT`]): when more than
//! that changed since the copy, it waits, and the app copies the changes again in the background
//! ([`catch_up_wanted`]), so the next start switches.
//!
//! Two copies of the app: each holds [`OPEN_LOCK_FILE`] in its data folder shared while it runs ([`hold_open_lock`]),
//! and the switch needs it alone, so a start while another copy is open never switches under it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::paths::{self, MOVED_IN_FILE};
use crate::songs::plural;

/// Beside the first data folder (`%LOCALAPPDATA%\MusicOrganiser-moving.txt`): the folder whose copy is ready, for the
/// next start to switch to. Removed once it has, or when the move is called off.
pub const PENDING_FILE: &str = "MusicOrganiser-moving.txt";
/// In the new folder while the move isn't finished: where from, and each file copied with the size and time it had.
pub const MOVING_IN_FILE: &str = "moving-in.json";
/// In the old folder once the switch is made: a note to say where the data went.
pub const MOVED_OUT_FILE: &str = "MOVED - the app now uses another folder.txt";
/// In the data folder: every running copy of the app holds it shared; the switch needs it alone. An empty file that
/// stays (like tidy's `tidy-notes.lock`).
pub const OPEN_LOCK_FILE: &str = "app-open.lock";
/// The app's own lock files, which only mean something while an app runs (and may be held locked): never copied.
pub const RUNNING_LOCKS: [&str; 3] = [OPEN_LOCK_FILE, crate::tidy::NOTES_LOCK_FILE, crate::note::WRITING_FILE];
/// Folders at the top of the data folder that are the launchers', not the app's data: never copied.
pub const LAUNCHER_FOLDERS: [&str; 2] = ["run-latest-rust", "run-latest"];
/// Files at the top of the data folder the app no longer opens, so not copied (the old databases once the Rust ones
/// replace them: they stay in the old folder for the archive step), each with the file that replaced it: one is left
/// behind only while that's there to go instead, so a folder without it keeps the old file. `sounds.db` (the C# app's
/// sound library, with its log) once `sounds-rust.db` has taken over (Solomon, 2026-11-05): with `sounds.db` gone, the
/// Sounds page takes the copy as it is for the app's own library ([`mo_sounds::compact::read_for_page`]).
pub const LEFT_BEHIND: &[(&str, &str)] = &[
    ("sounds.db", mo_sounds::compact::FILE_NAME),
    ("sounds.db-wal", mo_sounds::compact::FILE_NAME),
    ("sounds.db-shm", mo_sounds::compact::FILE_NAME),
];

/// Whether `name`, at the top of the data folder `dir`, is left behind ([`LEFT_BEHIND`]).
fn left_behind(dir: &Path, name: &str) -> bool {
    LEFT_BEHIND.iter().any(|&(old, instead)| old == name && dir.join(instead).is_file())
}
/// What a file is written as before it's checked.
const PART: &str = ".mo-move";
/// Room kept free on the new drive beyond the copy itself.
const SPARE_ROOM: u64 = 1 << 30;
/// A pass that copied less than this (and had nothing change under it) is the last before the switch.
const SMALL_PASS: u64 = 64 << 20;
const MAX_PASSES: usize = 3;
/// The most the switch at start copies before the window opens; more waits for a background catch-up.
pub const START_LIMIT: u64 = 1 << 30;
/// How often a pass saves what it has done so far, so a copy cut short goes on from there.
const SAVE_EVERY: Duration = Duration::from_secs(5);
const BUFFER: usize = 1 << 20;

/// A file's size and time, as copied.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stamp {
    pub size: u64,
    pub secs: u64,
    pub nanos: u32,
}

impl Stamp {
    fn of(meta: &fs::Metadata) -> Option<Stamp> {
        let time = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(Stamp { size: meta.len(), secs: time.as_secs(), nanos: time.subsec_nanos() })
    }

    fn time(self) -> SystemTime {
        UNIX_EPOCH + Duration::new(self.secs, self.nanos)
    }
}

/// [`MOVING_IN_FILE`] and, once switched, [`paths::MOVED_IN_FILE`] (without the file list).
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Record {
    /// The folder the data comes from.
    pub from: String,
    /// When the copy started and, once switched, when that was (local time, `2026-11-05 04:30`).
    #[serde(default)]
    pub started: String,
    #[serde(default)]
    pub switched: String,
    /// Each file copied, by its path in the folder (`/` between folders), as it was when copied.
    #[serde(default)]
    pub files: BTreeMap<String, Stamp>,
}

/// The finished move's record in `folder`, if the data moved into it.
pub fn moved_in(folder: &Path) -> Option<Record> {
    serde_json::from_str(&fs::read_to_string(folder.join(MOVED_IN_FILE)).ok()?).ok()
}

/// The folder a ready copy waits to switch to, beside the first data folder `first`.
pub fn pending_in(first: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(first.with_file_name(PENDING_FILE)).ok()?;
    Some(PathBuf::from(text.trim())).filter(|p| p.is_absolute())
}

/// The folder a ready copy waits to switch to at the next start.
pub fn pending() -> Option<PathBuf> {
    pending_in(&paths::first_dir())
}

/// Calls off a ready move: the next start doesn't switch. The copy stays where it is (yours to remove).
pub fn cancel() -> io::Result<()> {
    match fs::remove_file(paths::first_dir().with_file_name(PENDING_FILE)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => {
            set_start_note(None);
            Ok(())
        }
    }
}

/// What one pass over the folder did.
#[derive(Default, Debug)]
pub struct Pass {
    pub copied: usize,
    pub bytes: u64,
    /// Files that changed while copied, or were held locked: copied again next pass.
    pub busy: Vec<String>,
    /// Files that couldn't be copied, with why.
    pub failed: Vec<String>,
    /// What was left out (a link back into the folder, a name that isn't plain text) or copied through a link.
    pub notes: Vec<String>,
    /// With a limit: nothing was copied, as this much was to do.
    pub too_much: Option<u64>,
    /// Every file the copy now holds, and their size.
    pub files: usize,
    pub total: u64,
}

/// A file to copy: its path in the folder, where it is, how it looked.
struct Found {
    rel: String,
    path: PathBuf,
    stamp: Stamp,
}

/// Every file of the data folder `from` the move takes (see the module notes for what it leaves out and how links
/// are copied through). `inside` are folders a link must not lead into (the data folder and the copy). A file or
/// folder that goes while it's listed is passed over.
fn walk(from: &Path, inside: &[&Path], notes: &mut Vec<String>, stop: &AtomicBool) -> io::Result<Vec<Found>> {
    let mut found = Vec::new();
    let mut folders = vec![(from.to_path_buf(), String::new())];
    let mut through: BTreeSet<PathBuf> = BTreeSet::new();
    let real_inside: Vec<PathBuf> = inside.iter().map(|p| real(p)).collect();
    while let Some((dir, rel)) = folders.pop() {
        if stop.load(Ordering::Relaxed) {
            return Err(stopped());
        }
        let top = rel.is_empty();
        let listing = match fs::read_dir(&dir) {
            Ok(listing) => listing,
            Err(e) if !top && e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for entry in listing {
            let Ok(entry) = entry else { continue };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                notes.push(format!("left out {} (its name isn't plain text)", entry.path().display()));
                continue;
            };
            if top
                && (left_behind(&dir, &name)
                    || [MOVING_IN_FILE, MOVED_IN_FILE, MOVED_OUT_FILE, OPEN_LOCK_FILE].contains(&name.as_str()))
            {
                continue;
            }
            let here = if top { name.clone() } else { format!("{rel}/{name}") };
            let mut meta = match fs::symlink_metadata(entry.path()) {
                Ok(meta) => meta,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            if meta.file_type().is_symlink() {
                // Copied through: what it leads to, unless that's inside the data folder or the copy, or seen already.
                let Ok(target) = fs::metadata(entry.path()) else {
                    notes.push(format!("left out {here} (a link that leads nowhere)"));
                    continue;
                };
                let leads_to = real(&entry.path());
                if real_inside.iter().any(|f| mo_backup::folders::within(&leads_to, f))
                    || !through.insert(leads_to.clone())
                {
                    notes.push(format!("left out {here} (a link back into the folder)"));
                    continue;
                }
                notes.push(format!("{here} is a link to {}: its files were copied", leads_to.display()));
                meta = target;
            }
            if meta.is_dir() {
                if !(top && LAUNCHER_FOLDERS.iter().any(|f| f.eq_ignore_ascii_case(&name))) {
                    folders.push((entry.path(), here));
                }
            } else if meta.is_file() {
                if RUNNING_LOCKS.contains(&name.as_str()) || name.ends_with(PART) || name.ends_with(".mo-part") {
                    continue;
                }
                match Stamp::of(&meta) {
                    Some(stamp) => found.push(Found { rel: here, path: entry.path(), stamp }),
                    None => notes.push(format!("left out {here} (its time can't be read)")),
                }
            }
        }
    }
    found.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(found)
}

/// Every plain file in the copy `to`, by its path there, but the move's own notes at its top.
fn files_in(to: &Path) -> io::Result<Vec<String>> {
    let mut files = Vec::new();
    let mut folders = vec![(to.to_path_buf(), String::new())];
    while let Some((dir, rel)) = folders.pop() {
        let top = rel.is_empty();
        let listing = match fs::read_dir(&dir) {
            Ok(listing) => listing,
            Err(e) if !top && e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for entry in listing.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
            if top && [MOVING_IN_FILE, MOVED_IN_FILE, OPEN_LOCK_FILE].contains(&name.as_str()) {
                continue;
            }
            let here = if top { name.clone() } else { format!("{rel}/{name}") };
            match entry.file_type() {
                Ok(t) if t.is_dir() => folders.push((entry.path(), here)),
                Ok(t) if t.is_file() => files.push(here),
                _ => {}
            }
        }
    }
    Ok(files)
}

/// A file's place as it's matched: its case folded where the system ignores case (Windows, a Mac), so a file whose
/// name only changed case is one file, not one to copy and one to take out.
fn key(rel: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) { rel.to_lowercase() } else { rel.to_owned() }
}

/// Where `path` really is, through any links (as much of it as there is; the rest added as written).
fn real(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut at = path;
    loop {
        if let Ok(found) = fs::canonicalize(at) {
            return rest.iter().rev().fold(found, |p, part| p.join(part));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                at = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Where `rel` goes in `to`.
fn dest(to: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(to.to_path_buf(), |p, part| p.join(part))
}

/// Why one file wasn't copied.
enum NotCopied {
    /// It changed while being copied, or a running app holds it locked: try again.
    Busy,
    /// It went from the data folder since it was listed.
    Gone,
    Failed(io::Error),
}

impl From<io::Error> for NotCopied {
    fn from(e: io::Error) -> Self {
        if held_locked(&e) { NotCopied::Busy } else { NotCopied::Failed(e) }
    }
}

/// Windows: another program (or this one) holds the file or part of it locked.
fn held_locked(e: &io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33))
}

/// Copies one file, reading it once, then reads the copy back and checks both read the same, and that the source
/// didn't change meanwhile. The copy gets the source's time, then its name.
fn copy_one(f: &Found, to_file: &Path, stop: &AtomicBool) -> Result<(), NotCopied> {
    let mut part = to_file.as_os_str().to_owned();
    part.push(PART);
    let part = PathBuf::from(part);
    // One a stopped copy left: ours by its name.
    let _ = fs::remove_file(&part);
    let result = (|| {
        let mut src = match File::open(&f.path) {
            Ok(src) => src,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(NotCopied::Gone),
            Err(e) => return Err(e.into()),
        };
        let mut out = File::create_new(&part)?;
        let mut hash = Sha256::new();
        let mut buf = vec![0u8; BUFFER];
        let mut total = 0u64;
        while let Some(n) = read_some(&mut src, &mut buf, stop)? {
            hash.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            total += n as u64;
        }
        out.set_modified(f.stamp.time())?;
        out.sync_all()?;
        drop(out);
        drop(src);
        if total != f.stamp.size || fs::metadata(&f.path).ok().and_then(|m| Stamp::of(&m)) != Some(f.stamp) {
            return Err(NotCopied::Busy);
        }
        let wrote: [u8; 32] = hash.finalize().into();
        if hash_file(&part, stop)? != wrote {
            return Err(NotCopied::Failed(io::Error::other("the copy read back differently from what was read")));
        }
        fs::rename(&part, to_file)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

fn hash_file(path: &Path, stop: &AtomicBool) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; BUFFER];
    let mut hash = Sha256::new();
    while let Some(n) = read_some(&mut file, &mut buf, stop)? {
        hash.update(&buf[..n]);
    }
    Ok(hash.finalize().into())
}

fn read_some(file: &mut File, buf: &mut [u8], stop: &AtomicBool) -> io::Result<Option<usize>> {
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(stopped());
        }
        match file.read(buf) {
            Ok(0) => return Ok(None),
            Ok(n) => return Ok(Some(n)),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

fn stopped() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "Stopped")
}

fn is_stopped(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::Interrupted
}

/// One pass: copies each file of `from` the record doesn't have as it is now, then takes out of the copy every file
/// that isn't in `from` (gone since an earlier pass, or half made). `progress` hears the bytes done and to do. With a
/// `limit`, nothing is copied when more than that is to do ([`Pass::too_much`] says how much).
fn sync(
    from: &Path,
    to: &Path,
    record: &mut Record,
    limit: Option<u64>,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> io::Result<Pass> {
    let mut pass = Pass::default();
    let found = walk(from, &[from, to], &mut pass.notes, stop)?;
    let todo: Vec<&Found> = found
        .iter()
        .filter(|f| {
            record.files.get(&f.rel) != Some(&f.stamp)
                || fs::symlink_metadata(dest(to, &f.rel)).map_or(true, |m| !m.is_file() || m.len() != f.stamp.size)
        })
        .collect();
    let to_do: u64 = todo.iter().map(|f| f.stamp.size).sum();
    if limit.is_some_and(|limit| to_do > limit) {
        pass.too_much = Some(to_do);
        return Ok(pass);
    }
    progress(0, to_do);
    let mut gone: BTreeSet<String> = BTreeSet::new();
    let mut saved = Instant::now();
    for f in todo {
        let to_file = dest(to, &f.rel);
        if let Some(dir) = to_file.parent() {
            fs::create_dir_all(dir)?;
        }
        match copy_one(f, &to_file, stop) {
            Ok(()) => {
                record.files.insert(f.rel.clone(), f.stamp);
                pass.copied += 1;
                pass.bytes += f.stamp.size;
            }
            Err(NotCopied::Failed(e)) if is_stopped(&e) => return Err(e),
            Err(NotCopied::Busy) => {
                record.files.remove(&f.rel);
                pass.busy.push(f.rel.clone());
            }
            Err(NotCopied::Gone) => {
                record.files.remove(&f.rel);
                gone.insert(key(&f.rel));
            }
            Err(NotCopied::Failed(e)) => {
                record.files.remove(&f.rel);
                pass.failed.push(format!("{}: {e}", f.rel));
            }
        }
        progress(pass.bytes, to_do);
        if saved.elapsed() >= SAVE_EVERY {
            let _ = save_record(to, record);
            saved = Instant::now();
        }
    }
    // The copy holds only what the data folder holds: a file gone since, or half made, is taken out.
    let keep: BTreeSet<String> = found.iter().map(|f| key(&f.rel)).filter(|k| !gone.contains(k)).collect();
    for rel in files_in(to)? {
        if !keep.contains(&key(&rel)) {
            match fs::remove_file(dest(to, &rel)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
    }
    record.files.retain(|rel, _| keep.contains(&key(rel)));
    pass.files = keep.len();
    pass.total = found.iter().filter(|f| keep.contains(&key(&f.rel))).map(|f| f.stamp.size).sum();
    Ok(pass)
}

fn save_record(to: &Path, record: &Record) -> io::Result<()> {
    crate::backup::write_whole(
        &to.join(MOVING_IN_FILE),
        &serde_json::to_string_pretty(record).map_err(io::Error::other)?,
    )
}

fn now_text() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

/// Whether `to` may take the data of `from`: a whole path, neither inside the other (where each really is, through
/// links), and empty (or a copy of `from` this move started before). Gives what the copy holds so far.
fn check_folder(from: &Path, to: &Path) -> Result<Record, String> {
    if !to.is_absolute() {
        return Err(format!("{} isn't a whole folder path", to.display()));
    }
    let (real_from, real_to) = (real(from), real(to));
    let inside = |a: &Path, b: &Path| mo_backup::folders::within(a, b) || mo_backup::folders::within(b, a);
    if inside(to, from) || inside(&real_to, &real_from) {
        return Err("Pick a folder outside the app's data folder (and not one that holds it)".to_owned());
    }
    match fs::symlink_metadata(to) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(new_record(from)),
        Err(e) => return Err(format!("Couldn't look at {}: {e}", to.display())),
        Ok(m) if !m.is_dir() => return Err(format!("{} isn't a folder", to.display())),
        Ok(_) => {}
    }
    if let Ok(text) = fs::read_to_string(to.join(MOVING_IN_FILE)) {
        let record: Record =
            serde_json::from_str(&text).map_err(|e| format!("{MOVING_IN_FILE} in {} is damaged: {e}", to.display()))?;
        if !paths_match(Path::new(&record.from), from) {
            return Err(format!("{} holds a copy of another folder ({})", to.display(), record.from));
        }
        return Ok(record);
    }
    let empty = fs::read_dir(to).map_err(|e| format!("Couldn't look in {}: {e}", to.display()))?.next().is_none();
    if !empty {
        return Err(format!("{} isn't empty: pick an empty folder, or a new one", to.display()));
    }
    Ok(new_record(from))
}

fn new_record(from: &Path) -> Record {
    Record { from: from.display().to_string(), started: now_text(), ..Record::default() }
}

/// The free room on the drive `path` is on (or would be on), from its nearest folder there is.
pub(crate) fn free_room(path: &Path) -> Option<u64> {
    path.ancestors().find(|p| p.exists()).and_then(|p| fs4::available_space(p).ok())
}

/// What a finished copy did.
#[derive(Debug)]
pub struct Copied {
    pub files: usize,
    pub bytes: u64,
    /// Files still changing, which the switch copies at the next start.
    pub at_start: usize,
    /// What was left out, or copied through a link.
    pub notes: Vec<String>,
}

/// Step 1 and 2: copies the data folder in use, `from`, to `to` and checks it, then marks it ready for the next start.
/// Runs for minutes on a big library: call it on a background thread. `progress` hears the bytes done and to do in
/// each pass. Stops (with `Err("Stopped")`) when `stop` is set; the copy so far stays, and a later call goes on from
/// there.
pub fn copy(from: &Path, to: &Path, stop: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<Copied, String> {
    let copied = copy_with(from, to, &paths::first_dir(), stop, progress);
    if copied.is_ok() {
        set_start_note(None);
    }
    copied
}

fn copy_with(
    from: &Path,
    to: &Path,
    first: &Path,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Copied, String> {
    let mut record = check_folder(from, to)?;
    let fail = |e: io::Error| if is_stopped(&e) { "Stopped".to_owned() } else { e.to_string() };
    let mut notes = Vec::new();
    let all = walk(from, &[from, to], &mut notes, stop).map_err(fail)?;
    let needed: u64 = all.iter().filter(|f| record.files.get(&f.rel) != Some(&f.stamp)).map(|f| f.stamp.size).sum();
    if let Some(free) = free_room(to)
        && free < needed.saturating_add(SPARE_ROOM)
    {
        return Err(format!(
            "Not enough room: the copy needs {} and {} has {} free (with 1 GB to spare)",
            size_text(needed),
            to.display(),
            size_text(free)
        ));
    }
    fs::create_dir_all(to).map_err(|e| format!("Couldn't make {}: {e}", to.display()))?;
    save_record(to, &record).map_err(|e| format!("Couldn't write in {}: {e}", to.display()))?;
    let mut last = Pass::default();
    for _ in 0..MAX_PASSES {
        let pass = sync(from, to, &mut record, None, stop, progress);
        // What the pass did is kept even when it stopped part way.
        save_record(to, &record).map_err(|e| format!("Couldn't write in {}: {e}", to.display()))?;
        last = pass.map_err(fail)?;
        if last.failed.is_empty() && last.busy.is_empty() && last.bytes < SMALL_PASS {
            break;
        }
    }
    if !last.failed.is_empty() {
        return Err(format!("{} couldn't be copied: {}", plural(last.failed.len(), "file"), first_few(&last.failed)));
    }
    crate::backup::write_whole(&first.with_file_name(PENDING_FILE), &to.display().to_string())
        .map_err(|e| format!("Couldn't note the move beside {}: {e}", first.display()))?;
    Ok(Copied { files: last.files, bytes: last.total, at_start: last.busy.len(), notes: last.notes })
}

/// What the start did about a ready move.
#[derive(Debug)]
pub enum AtStart {
    /// The app now uses `to`.
    Switched { from: PathBuf, to: PathBuf, copied: usize, bytes: u64, took: Duration, notes: Vec<String> },
    /// Not this time; tried again at the next start. `catch_up`: too much changed since the copy to take on before
    /// the window opens, so the app copies it again in the background now.
    NotYet { to: PathBuf, why: String, catch_up: bool },
    /// Called off: the ready copy can't be used.
    Dropped { to: PathBuf, why: String },
    /// Another copy of the app was switching folders as this one started: this one waited for it to finish, and
    /// there was nothing left to do after it, so it opens on whichever folder that copy left in use.
    Waited { to: PathBuf, took: Duration },
}

impl AtStart {
    /// The session log's line.
    pub fn line(&self) -> String {
        match self {
            AtStart::Switched { from, to, copied, bytes, took, notes } => {
                let mut line = format!(
                    "Now using {}, moved from {} (the last {} caught up, {}, in {:.1} s); the old folder stays",
                    to.display(),
                    from.display(),
                    plural(*copied, "file"),
                    size_text(*bytes),
                    took.as_secs_f64()
                );
                if !notes.is_empty() {
                    line.push_str(&format!("; {}", notes.join("; ")));
                }
                line
            }
            AtStart::NotYet { to, why, .. } => format!("Not switching to {} yet: {why}", to.display()),
            AtStart::Dropped { to, why } => format!("The move to {} is called off: {why}", to.display()),
            AtStart::Waited { to, took } => format!(
                "Another copy of the app was switching to {}: waited {:.1} s for it to finish",
                to.display(),
                took.as_secs_f64()
            ),
        }
    }
}

/// What the start said about a move that didn't switch, for Settings, and the folder to catch up in the background.
struct StartNote {
    line: String,
    catch_up: Option<PathBuf>,
}

static START_NOTE: Mutex<Option<StartNote>> = Mutex::new(None);

fn set_start_note(note: Option<StartNote>) {
    *START_NOTE.lock().unwrap_or_else(PoisonError::into_inner) = note;
}

/// Why the start didn't switch to a ready move, for Settings, until the move is called off or copied again.
pub fn start_problem() -> Option<String> {
    START_NOTE.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|n| n.line.clone())
}

/// The folder to copy the changes to again in the background, when the start found too much changed to switch.
/// Asked once: the next ask gives `None`.
pub fn catch_up_wanted() -> Option<PathBuf> {
    START_NOTE.lock().unwrap_or_else(PoisonError::into_inner).as_mut().and_then(|n| n.catch_up.take())
}

/// Step 3, first thing at start, before any of the app's files is opened (and before [`paths::data_dir`] is asked):
/// when a copy is ready, catches it up and switches to it. `None` when there's no move to make.
pub fn finish_at_start() -> Option<AtStart> {
    let outcome = finish_in(&paths::first_dir());
    set_start_note(match &outcome {
        Some(o @ AtStart::NotYet { to, catch_up, .. }) => {
            Some(StartNote { line: o.line(), catch_up: catch_up.then(|| to.clone()) })
        }
        Some(o @ AtStart::Dropped { .. }) => Some(StartNote { line: o.line(), catch_up: None }),
        _ => None,
    });
    outcome
}

fn finish_in(first: &Path) -> Option<AtStart> {
    let to = pending_in(first)?;
    let in_use = paths::resolve_in(first);
    let from = in_use.folder;
    let pending_file = first.with_file_name(PENDING_FILE);
    let call_off = |why: String| {
        let _ = fs::remove_file(&pending_file);
        Some(AtStart::Dropped { to: to.clone(), why })
    };
    let not_yet = |why: String| Some(AtStart::NotYet { to: to.clone(), why, catch_up: false });
    if paths_match(&from, &to) {
        let _ = fs::remove_file(&pending_file);
        return None;
    }
    if !to.is_dir() {
        return not_yet("the folder can't be found (is its drive there?)".to_owned());
    }
    let mut record: Record = match fs::read_to_string(to.join(MOVING_IN_FILE)).map(|t| serde_json::from_str(&t)) {
        Ok(Ok(record)) => record,
        Ok(Err(e)) => return call_off(format!("its {MOVING_IN_FILE} is damaged ({e})")),
        Err(e) => return call_off(format!("its {MOVING_IN_FILE} can't be read ({e})")),
    };
    if !paths_match(Path::new(&record.from), &from) {
        // The folder in use isn't the one copied: when that's only because a moved folder's drive is missing now,
        // wait for it; otherwise the copy is of a folder the app no longer uses.
        if in_use.problem.is_some() {
            return not_yet(format!("the folder it was copied from ({}) can't be found", record.from));
        }
        return call_off(format!("it's a copy of {}, but the app now uses {}", record.from, from.display()));
    }
    // No other copy of the app may be open on the old folder while it's caught up and left.
    let Ok(lock) = File::options().create(true).truncate(false).write(true).open(from.join(OPEN_LOCK_FILE)) else {
        return not_yet(format!("{OPEN_LOCK_FILE} couldn't be opened"));
    };
    match lock.try_lock() {
        Ok(()) => {}
        // Held shared: another copy of the app is open on this folder, so not now. Held alone: another copy is
        // switching folders right now (F-1), so wait for it, then open on whatever folder it left in use.
        // (A copy that closes or opens between the two asks only makes this say "not now": the next start tries again.)
        Err(fs::TryLockError::WouldBlock) => {
            let started = Instant::now();
            return match lock.try_lock_shared() {
                Ok(()) => {
                    let _ = lock.unlock();
                    not_yet("another copy of the app is open".to_owned())
                }
                Err(fs::TryLockError::WouldBlock) => {
                    if !wait_for_switch(&lock) {
                        return not_yet(format!(
                            "another copy of the app was still switching folders after {} s",
                            SWITCH_WAIT.as_secs()
                        ));
                    }
                    // It's done: look again, as that copy may have switched, called the move off or left it for later.
                    drop(lock);
                    finish_in(first).or(Some(AtStart::Waited { to: to.clone(), took: started.elapsed() }))
                }
                Err(fs::TryLockError::Error(e)) => not_yet(format!("{OPEN_LOCK_FILE} couldn't be locked ({e})")),
            };
        }
        Err(fs::TryLockError::Error(e)) => return not_yet(format!("{OPEN_LOCK_FILE} couldn't be locked ({e})")),
    }
    let started = Instant::now();
    let never = AtomicBool::new(false);
    let pass = sync(&from, &to, &mut record, Some(START_LIMIT), &never, &mut |_, _| {});
    if save_record(&to, &record).is_err() {
        return not_yet(format!("couldn't write {MOVING_IN_FILE}"));
    }
    let pass = match pass {
        Ok(pass) => pass,
        Err(e) => return not_yet(format!("the last copy stopped: {e}")),
    };
    if let Some(much) = pass.too_much {
        return Some(AtStart::NotYet {
            to: to.clone(),
            why: format!("{} changed since the copy, so it's copied again in the background first", size_text(much)),
            catch_up: true,
        });
    }
    if !pass.failed.is_empty() {
        return not_yet(format!(
            "{} couldn't be copied: {}",
            plural(pass.failed.len(), "file"),
            first_few(&pass.failed)
        ));
    }
    if !pass.busy.is_empty() {
        return not_yet(format!("{} kept changing: {}", plural(pass.busy.len(), "file"), first_few(&pass.busy)));
    }
    // Every file is there and checked: the new folder is marked a finished move, then the pointer names it.
    let moved = Record { files: BTreeMap::new(), switched: now_text(), ..record };
    let marked = serde_json::to_string_pretty(&moved)
        .map_err(io::Error::other)
        .and_then(|json| crate::backup::write_whole(&to.join(MOVED_IN_FILE), &json));
    if let Err(e) = marked {
        return not_yet(format!("couldn't write {MOVED_IN_FILE} ({e})"));
    }
    if let Err(e) = crate::backup::write_whole(&paths::pointer_file(first), &to.display().to_string()) {
        return not_yet(format!("couldn't write {} ({e})", paths::POINTER_FILE));
    }
    let _ = fs::remove_file(to.join(MOVING_IN_FILE));
    let _ = fs::remove_file(&pending_file);
    let _ = fs::write(from.join(MOVED_OUT_FILE), moved_out_note(&from, first, &to, &moved.switched));
    let _ = lock.unlock();
    Some(AtStart::Switched {
        from,
        to,
        copied: pass.copied,
        bytes: pass.bytes,
        took: started.elapsed(),
        notes: pass.notes,
    })
}

/// Waits (at most [`SWITCH_WAIT`]) until no other copy of the app holds `lock` alone, as one does only while it
/// catches up and switches folders. Whether it let go in time.
fn wait_for_switch(lock: &File) -> bool {
    let started = Instant::now();
    while started.elapsed() < SWITCH_WAIT {
        match lock.try_lock_shared() {
            Ok(()) => {
                let _ = lock.unlock();
                return true;
            }
            Err(fs::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(250)),
            Err(fs::TryLockError::Error(_)) => return false,
        }
    }
    false
}

/// The longest a start waits, before its window opens, for another copy of the app that is switching folders: a
/// switch catches up at most [`START_LIMIT`], under a minute on a slow drive. Longer, and this start opens on the old
/// folder and the move waits for the next one.
const SWITCH_WAIT: Duration = Duration::from_secs(90);

/// The note left in the folder the data moved out of, `from` (to `to`, at `when`). What to keep there: the launcher
/// and the pointer only when it's the system's own folder `first` (a second move, out of a folder a move went to,
/// has neither), and the old files the copy left behind for the archive step, as there are any.
fn moved_out_note(from: &Path, first: &Path, to: &Path, when: &str) -> String {
    let mut keep = Vec::new();
    let is_first = paths_match(from, first);
    if is_first {
        let launchers: Vec<&str> = LAUNCHER_FOLDERS.iter().copied().filter(|f| from.join(f).is_dir()).collect();
        if !launchers.is_empty() {
            keep.push(format!("{}: the launcher, which the Desktop shortcut starts", launchers.join(" and ")));
        }
    }
    let left: Vec<&str> = LEFT_BEHIND
        .iter()
        .map(|&(old, _)| old)
        .filter(|old| from.join(old).is_file() && left_behind(from, old))
        .collect();
    if !left.is_empty() {
        keep.push(format!("{}: not copied, kept here until the app has archived them", left.join(", ")));
    }
    let mut note = format!(
        "Music Organiser copied its data from this folder to\r\n\r\n    {}\r\n\r\non {when} and uses that folder now. \
         Nothing here was changed or removed.\r\n",
        to.display()
    );
    if !keep.is_empty() {
        note.push_str("\r\nKeep in this folder:\r\n");
        for k in &keep {
            note.push_str(&format!("  - {k}\r\n"));
        }
    }
    if is_first {
        note.push_str(&format!(
            "\r\nKeep {} beside this folder too: it tells the app where its data is now.\r\n",
            paths::POINTER_FILE
        ));
    }
    note.push_str("\r\nThe rest can go once you're happy the app works from the new folder.\r\n");
    note
}

/// Whether two folder paths name the same folder as written (Windows and a Mac ignore case).
fn paths_match(a: &Path, b: &Path) -> bool {
    mo_backup::folders::within(a, b) && mo_backup::folders::within(b, a)
}

/// Holds [`OPEN_LOCK_FILE`] in the data folder shared for the rest of the run, so a second copy of the app started
/// meanwhile never switches folders under this one. Call once, after [`finish_at_start`].
pub fn hold_open_lock() {
    static HELD: OnceLock<File> = OnceLock::new();
    let path = paths::data_dir().join(OPEN_LOCK_FILE);
    let held = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("couldn't open it ({e})"))
        .and_then(|file| file.try_lock_shared().map(|()| file).map_err(|e| e.to_string()));
    match held {
        Ok(file) => {
            let _ = HELD.set(file);
        }
        Err(e) => crate::log::warning(
            "Data folder",
            &format!(
                "Couldn't hold {OPEN_LOCK_FILE} ({e}): another copy started now could switch folders under this one"
            ),
        ),
    }
}

fn first_few(list: &[String]) -> String {
    let mut text = list.iter().take(3).cloned().collect::<Vec<_>>().join("; ");
    if list.len() > 3 {
        text.push_str(&format!(" and {} more", list.len() - 3));
    }
    text
}

/// A size as the Space page shows it.
pub fn size_text(bytes: u64) -> String {
    mo_space::size_text(bytes)
}

/// The line Settings shows under the data folder: a move waiting for the next start, where the data came from, or
/// why a move's folder isn't in use.
pub fn status_note(pending: Option<&Path>, moved_from: Option<&Path>, problems: &[String]) -> String {
    let mut lines: Vec<String> = problems.to_vec();
    if let Some(to) = pending {
        lines.push(format!(
            "Copied and checked to {}. The app switches to it the next time it starts; until then it keeps using this \
             folder.",
            to.display()
        ));
    } else if let Some(from) = moved_from {
        let mut line = format!("Moved here from {}. That folder stays as it was.", from.display());
        if paths_match(from, &paths::first_dir()) {
            line.push_str(&format!(
                " Keep its {} folders (the launcher and its Desktop shortcut) and {} beside it.",
                LAUNCHER_FOLDERS.join(" and "),
                paths::POINTER_FILE
            ));
        }
        let left: Vec<&str> = LEFT_BEHIND
            .iter()
            .map(|&(old, _)| old)
            .filter(|old| from.join(old).is_file() && left_behind(from, old))
            .collect();
        if !left.is_empty() {
            line.push_str(&format!(" Keep {} there until the app has archived it.", left.join(", ")));
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// The folder a move goes into when `picked` is chosen: a folder of the app's own name inside it, unless it's one
/// already (so picking `I:\` moves to `I:\MusicOrganiser`).
pub fn target_in(picked: &Path) -> PathBuf {
    if picked.file_name().is_some_and(|n| n.eq_ignore_ascii_case("MusicOrganiser")) {
        picked.to_path_buf()
    } else {
        picked.join("MusicOrganiser")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A first data folder and a place to move it to, in a test folder of its own.
    struct Setup {
        root: PathBuf,
        first: PathBuf,
        to: PathBuf,
    }

    impl Setup {
        fn new(name: &str) -> Setup {
            let root = std::env::temp_dir().join("mo-data-move-test").join(format!("{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let first = root.join("MusicOrganiser");
            fs::create_dir_all(first.join("logs")).unwrap();
            fs::create_dir_all(first.join("notes")).unwrap();
            fs::write(first.join("library.db"), vec![7u8; 300_000]).unwrap();
            fs::write(first.join("library.db-wal"), b"wal").unwrap();
            fs::write(first.join("library.db-shm"), b"shm").unwrap();
            fs::write(first.join("rust-settings.json"), b"{}").unwrap();
            fs::write(first.join("logs").join("session.log"), b"line\n").unwrap();
            fs::write(first.join("notes").join("draft.txt"), b"words").unwrap();
            fs::write(first.join("notes").join("writing.lock"), b"123").unwrap();
            fs::create_dir_all(first.join("keyrefit").join("repo")).unwrap();
            fs::write(first.join("keyrefit").join("repo").join("Cargo.lock"), b"# lock").unwrap();
            fs::create_dir_all(first.join("run-latest-rust").join("app")).unwrap();
            fs::write(first.join("run-latest-rust").join("app").join("Cargo.toml"), b"x").unwrap();
            let to = root.join("I").join("MusicOrganiser");
            Setup { root, first, to }
        }

        fn copy(&self) -> Result<Copied, String> {
            copy_with(&self.first, &self.to, &self.first, &AtomicBool::new(false), &mut |_, _| {})
        }
    }

    impl Drop for Setup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn copies_checks_then_switches_at_the_next_start() {
        let s = Setup::new("whole");
        let copied = s.copy().unwrap();
        assert_eq!(copied.files, 7);
        // Ready, not switched: the app still uses the first folder.
        assert_eq!(pending_in(&s.first), Some(s.to.clone()));
        assert_eq!(paths::folder_in_use(&s.first), s.first);
        assert_eq!(fs::read(s.to.join("library.db")).unwrap(), vec![7u8; 300_000]);
        // Times kept, so every stamp by size and time stays good.
        let time = |p: &Path| fs::metadata(p).unwrap().modified().unwrap();
        assert_eq!(time(&s.to.join("library.db")), time(&s.first.join("library.db")));
        // The database's -wal and -shm go with it; the launcher's folder and lock files don't.
        assert!(s.to.join("library.db-shm").is_file());
        assert!(!s.to.join("run-latest-rust").exists());
        assert!(!s.to.join("notes").join("writing.lock").exists());
        // Another program's .lock file (a tool's Cargo.lock) is data like any other.
        assert!(s.to.join("keyrefit").join("repo").join("Cargo.lock").is_file());

        // The last run changed a setting and wrote its log, and a note went.
        fs::write(s.first.join("rust-settings.json"), b"{\"theme\":\"Paper\"}").unwrap();
        fs::write(s.first.join("logs").join("session-2.log"), b"more\n").unwrap();
        fs::remove_file(s.first.join("notes").join("draft.txt")).unwrap();

        let Some(AtStart::Switched { copied, .. }) = finish_in(&s.first) else { panic!("didn't switch") };
        assert_eq!(copied, 2);
        assert_eq!(paths::folder_in_use(&s.first), s.to);
        assert_eq!(fs::read(s.to.join("rust-settings.json")).unwrap(), b"{\"theme\":\"Paper\"}");
        assert!(s.to.join("logs").join("session-2.log").is_file());
        assert!(!s.to.join("notes").join("draft.txt").exists());
        assert!(!s.to.join(MOVING_IN_FILE).exists());
        assert_eq!(moved_in(&s.to).unwrap().from, s.first.display().to_string());
        assert!(pending_in(&s.first).is_none());
        // The old folder keeps everything, and says where the data went.
        assert!(s.first.join("library.db").is_file());
        assert!(fs::read_to_string(s.first.join(MOVED_OUT_FILE)).unwrap().contains(&s.to.display().to_string()));
        // Nothing more to do.
        assert!(finish_in(&s.first).is_none());
    }

    #[test]
    fn sounds_db_stays_behind_once_the_apps_own_sound_library_is_there() {
        let s = Setup::new("sounds");
        for name in ["sounds.db", "sounds.db-wal", "sounds.db-shm"] {
            fs::write(s.first.join(name), b"C#").unwrap();
        }
        // No sounds-rust.db yet: sounds.db is the only sound library, so it goes.
        assert_eq!(s.copy().unwrap().files, 10);
        assert!(s.to.join("sounds.db").is_file());
        // There: sounds.db stays in the old folder for the archive step (and leaves the copy, as a file gone does).
        fs::write(s.first.join(mo_sounds::compact::FILE_NAME), b"Rust").unwrap();
        s.copy().unwrap();
        assert!(s.to.join(mo_sounds::compact::FILE_NAME).is_file());
        for name in ["sounds.db", "sounds.db-wal", "sounds.db-shm"] {
            assert!(!s.to.join(name).exists(), "{name}");
            assert!(s.first.join(name).is_file(), "{name}");
        }
    }

    #[test]
    fn a_folder_that_isnt_empty_or_is_inside_is_refused() {
        let s = Setup::new("refused");
        fs::create_dir_all(&s.to).unwrap();
        fs::write(s.to.join("yours.txt"), b"x").unwrap();
        assert!(s.copy().unwrap_err().contains("isn't empty"));
        let inside = s.first.join("inner");
        let err = copy_with(&s.first, &inside, &s.first, &AtomicBool::new(false), &mut |_, _| {}).unwrap_err();
        assert!(err.contains("outside"));
        assert!(pending_in(&s.first).is_none());
        assert!(fs::read_to_string(s.to.join("yours.txt")).is_ok());
    }

    #[test]
    fn a_stopped_copy_goes_on_where_it_was() {
        let s = Setup::new("stopped");
        let stop = AtomicBool::new(true);
        let err = copy_with(&s.first, &s.to, &s.first, &stop, &mut |_, _| {}).unwrap_err();
        assert_eq!(err, "Stopped");
        assert!(pending_in(&s.first).is_none());
        // Its own folder now (it holds the record), so going on is allowed.
        let copied = s.copy().unwrap();
        assert_eq!(copied.files, 7);
    }

    #[test]
    fn a_damaged_copy_is_never_switched_to() {
        let s = Setup::new("damaged");
        s.copy().unwrap();
        // The copy's list went missing: called off, never switched.
        fs::remove_file(s.to.join(MOVING_IN_FILE)).unwrap();
        assert!(matches!(finish_in(&s.first), Some(AtStart::Dropped { .. })));
        assert_eq!(paths::folder_in_use(&s.first), s.first);
        assert!(pending_in(&s.first).is_none());
    }

    #[test]
    fn a_missing_drive_waits() {
        let s = Setup::new("missing");
        s.copy().unwrap();
        fs::rename(&s.to, s.root.join("away")).unwrap();
        assert!(matches!(finish_in(&s.first), Some(AtStart::NotYet { .. })));
        assert_eq!(pending_in(&s.first), Some(s.to.clone()));
        assert_eq!(paths::folder_in_use(&s.first), s.first);
    }

    #[test]
    fn a_copy_that_differs_is_caught_up() {
        let s = Setup::new("differs");
        s.copy().unwrap();
        // Someone changed the copy itself: its size no longer matches, so it's copied again.
        fs::write(s.to.join("library.db"), b"short").unwrap();
        assert!(matches!(finish_in(&s.first), Some(AtStart::Switched { copied: 1, .. })));
        assert_eq!(fs::read(s.to.join("library.db")).unwrap().len(), 300_000);
    }

    #[test]
    fn another_open_copy_of_the_app_holds_the_switch() {
        let s = Setup::new("open");
        s.copy().unwrap();
        let other =
            File::options().create(true).truncate(false).write(true).open(s.first.join(OPEN_LOCK_FILE)).unwrap();
        other.lock_shared().unwrap();
        assert!(matches!(finish_in(&s.first), Some(AtStart::NotYet { .. })));
        drop(other);
        assert!(matches!(finish_in(&s.first), Some(AtStart::Switched { .. })));
    }

    #[test]
    fn a_file_gone_since_an_earlier_pass_leaves_the_copy_too() {
        let s = Setup::new("gone");
        s.copy().unwrap();
        // The old log isn't in the record now (it was busy last pass), and the app's last close took it away: the
        // copy must not keep the stale one (SQLite would play an old -wal over newer pages).
        let mut record: Record = serde_json::from_str(&fs::read_to_string(s.to.join(MOVING_IN_FILE)).unwrap()).unwrap();
        record.files.remove("library.db-wal");
        save_record(&s.to, &record).unwrap();
        fs::remove_file(s.first.join("library.db-wal")).unwrap();
        // A half-made copy a killed run left goes too.
        fs::write(s.to.join(format!("rust-settings.json{PART}")), b"half").unwrap();
        assert!(matches!(finish_in(&s.first), Some(AtStart::Switched { .. })));
        assert!(!s.to.join("library.db-wal").exists());
        assert!(!s.to.join(format!("rust-settings.json{PART}")).exists());
        assert!(s.to.join("library.db").is_file());
    }

    #[test]
    fn too_much_changed_waits_for_a_catch_up() {
        let s = Setup::new("much");
        s.copy().unwrap();
        let mut record: Record = serde_json::from_str(&fs::read_to_string(s.to.join(MOVING_IN_FILE)).unwrap()).unwrap();
        let sync_it = |record: &mut Record, limit| {
            sync(&s.first, &s.to, record, Some(limit), &AtomicBool::new(false), &mut |_, _| {}).unwrap()
        };
        fs::write(s.first.join("library.db"), vec![8u8; 300_000]).unwrap();
        let pass = sync_it(&mut record, 1000);
        assert_eq!((pass.too_much, pass.copied), (Some(300_000), 0));
        let pass = sync_it(&mut record, START_LIMIT);
        assert_eq!((pass.too_much, pass.copied), (None, 1));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_copied_through_unless_it_leads_back_in() {
        use std::os::unix::fs::symlink;
        let s = Setup::new("links");
        let away = s.root.join("D").join("logs");
        fs::create_dir_all(&away).unwrap();
        fs::write(away.join("old.log"), b"kept").unwrap();
        fs::remove_dir_all(s.first.join("logs")).unwrap();
        symlink(&away, s.first.join("logs")).unwrap();
        symlink(&s.first, s.first.join("loop")).unwrap();
        let copied = s.copy().unwrap();
        assert_eq!(fs::read(s.to.join("logs").join("old.log")).unwrap(), b"kept");
        assert!(fs::symlink_metadata(s.to.join("logs")).unwrap().is_dir());
        assert!(!s.to.join("loop").exists());
        assert!(copied.notes.iter().any(|n| n.contains("back into the folder")));
        // A target reached through a link into the data folder is refused.
        let into = s.root.join("into");
        symlink(&s.first, &into).unwrap();
        let err = copy_with(&s.first, &into.join("MusicOrganiser"), &s.first, &AtomicBool::new(false), &mut |_, _| {});
        assert!(err.unwrap_err().contains("outside"));
    }

    #[test]
    fn a_start_while_another_copy_switches_waits_for_it() {
        let s = Setup::new("waits");
        s.copy().unwrap();
        // Another copy is switching: it holds the lock alone.
        let other =
            File::options().create(true).truncate(false).write(true).open(s.first.join(OPEN_LOCK_FILE)).unwrap();
        other.lock().unwrap();
        let first = s.first.clone();
        let start = std::thread::spawn(move || finish_in(&first));
        std::thread::sleep(Duration::from_millis(600));
        assert!(!start.is_finished());
        // It let go without switching: this start switches itself.
        other.unlock().unwrap();
        assert!(matches!(start.join().unwrap(), Some(AtStart::Switched { .. })));

        // It switched (the move is no longer waiting) before letting go: nothing left for this start to do.
        let s = Setup::new("waits-done");
        s.copy().unwrap();
        let other =
            File::options().create(true).truncate(false).write(true).open(s.first.join(OPEN_LOCK_FILE)).unwrap();
        other.lock().unwrap();
        let first = s.first.clone();
        let start = std::thread::spawn(move || finish_in(&first));
        std::thread::sleep(Duration::from_millis(600));
        fs::remove_file(s.first.with_file_name(PENDING_FILE)).unwrap();
        other.unlock().unwrap();
        assert!(matches!(start.join().unwrap(), Some(AtStart::Waited { .. })));
    }

    #[test]
    fn the_moved_out_note_names_only_what_that_folder_has() {
        let s = Setup::new("note");
        fs::write(s.first.join("sounds.db"), b"old").unwrap();
        fs::write(s.first.join(mo_sounds::compact::FILE_NAME), b"new").unwrap();
        let note = moved_out_note(&s.first, &s.first, &s.to, "now");
        assert!(note.contains("run-latest-rust:") && note.contains(paths::POINTER_FILE));
        assert!(note.contains("sounds.db:") && !note.contains("sounds.db-wal"));
        // A second move, out of a folder a move went to: no launcher or pointer there.
        let note = moved_out_note(&s.to, &s.first, &s.root.join("G"), "now");
        assert!(!note.contains("run-latest") && !note.contains(paths::POINTER_FILE) && !note.contains("Keep in"));
    }
}
