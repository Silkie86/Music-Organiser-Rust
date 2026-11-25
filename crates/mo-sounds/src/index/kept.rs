//! An index's numbers kept in a file beside the app's data and read as they're needed, not held in the app's own
//! memory (Solomon, 2026-11-25: CLAP's were 465 MB of the app's memory for his half a million sounds). Two kinds of
//! file ([`Number`]): CLAP's half-precision numbers (`sound-likeness-…`), and the analyser's fingerprints at full
//! precision (`sound-prints-…`), each swept and replaced on its own.
//!
//! The file is read with ordinary reads, a part of the rows at a time, into a small buffer: Windows keeps what's
//! been read in its own file cache while it has room, and lets go of it when it's short without writing it to the
//! page file. It isn't mapped into the app's memory: a mapped file that can't be read (the drive gone, a bad sector)
//! stops the whole program with nothing logged, where a read that fails just says so. A search that can't read
//! finds nothing, and the trouble is reported once ([`on_trouble`]).
//!
//! The file's name says what's in it (a hash of its numbers), so:
//! - a library whose fingerprints changed gets a file of another name, and a file in use is never written to;
//! - a file of the right name is still checked, number for number, against the ones just read from the library,
//!   before it's trusted.
//!
//! It's written whole under another name first (ending `.mo-part`, as the app's other half-written files), flushed
//! to the disk, read back and checked, and only then given its name. Files of this kind that aren't the one in use
//! are removed (rule 28), before the new one is written, so they never take its room: only plain files whose whole
//! name is one this code gives ([`is_kept_name`]), never through a link, and a half-written one only once it's an hour
//! old (another copy of the app may be writing it now; the app's start-up sweep clears those too,
//! [`is_half_kept_name`]). The plugin never comes here: it writes none of the app's files.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// A kind of number an index holds, and the kind of file its numbers are kept in.
pub(super) trait Number: bytemuck::Pod + PartialEq + Send + Sync {
    /// What every file of the kind starts with: which kind of file, and which layout.
    const MAGIC: &'static [u8; 8];
    /// What every file of the kind's name starts with.
    const START: &'static str;
}

/// CLAP's numbers, at half precision.
impl Number for u16 {
    const MAGIC: &'static [u8; 8] = b"MOLIKE1\n";
    const START: &'static str = "sound-likeness-";
}

/// The analyser's fingerprints, at full precision.
impl Number for f32 {
    const MAGIC: &'static [u8; 8] = b"MOPRNT1\n";
    const START: &'static str = "sound-prints-";
}

/// The starts of every kind's names.
const STARTS: [&str; 2] = [<u16 as Number>::START, <f32 as Number>::START];
/// The numbers start here: a whole page in, so they lie as the processor wants them.
const HEADER: usize = 4096;
const END: &str = ".rows";
const PART: &str = ".mo-part";
/// How much room the drive must have left over after the file is written: this, or as much again as the file for a
/// file smaller than it.
const SPARE: u64 = 1 << 30;
/// A half-written file older than this was left by a run that stopped.
const LEFT_BEHIND: Duration = Duration::from_secs(3600);
/// How much is read or compared at a time.
const PIECE: usize = 1 << 20;

/// What keeping an index's numbers in a file did, for the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    /// The file they're in.
    pub file: PathBuf,
    /// Its size in bytes: what the app no longer holds itself.
    pub bytes: u64,
    /// Whether the file was written now (else one from an earlier run was found whole and used).
    pub written: bool,
    /// How many older files of this kind were removed.
    pub removed: usize,
}

/// Who hears when a kept file can't be read ([`on_trouble`]).
static TROUBLE: OnceLock<fn(&str)> = OnceLock::new();

/// From now on `say` is told, once for each file, when a kept file can't be read (the app logs it). The first
/// caller's stays.
pub fn on_trouble(say: fn(&str)) {
    let _ = TROUBLE.set(say);
}

/// Numbers in a file ([`keep`]), read as they're asked for.
pub(super) struct InFile {
    file: File,
    path: PathBuf,
    /// How many numbers it holds.
    numbers: usize,
    /// How many bytes each number takes.
    size: usize,
    /// Whether a failed read has been reported (once is enough).
    told: AtomicBool,
}

impl InFile {
    /// Fills `out` with the numbers from the `at`th on, as the heap held them (every row one after another). Says
    /// no when they can't be read (and reports it the first time): `out` is then not to be used.
    pub(super) fn read<T: Number>(&self, at: usize, out: &mut [T]) -> bool {
        let read = if size_of::<T>() != self.size {
            Err(std::io::Error::other("it holds another kind of number"))
        } else if at.checked_add(out.len()).is_none_or(|end| end > self.numbers) {
            Err(std::io::Error::other("past the end of the file"))
        } else {
            read_exact_at(&self.file, bytemuck::cast_slice_mut::<T, u8>(out), (HEADER + at * self.size) as u64)
        };
        match read {
            Ok(()) => true,
            Err(e) => {
                if !self.told.swap(true, Ordering::Relaxed)
                    && let Some(say) = TROUBLE.get()
                {
                    say(&format!("{} couldn't be read ({e})", self.path.display()));
                }
                false
            }
        }
    }

    /// How many numbers it holds.
    pub(super) fn len(&self) -> usize {
        self.numbers
    }
}

/// Fills `out` from `file` at `offset`, wherever other threads are reading it.
fn read_exact_at(file: &File, mut out: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    while !out.is_empty() {
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(file, out, offset)?;
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(file, out, offset)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        out = &mut out[n..];
        offset += n as u64;
    }
    Ok(())
}

impl std::fmt::Debug for InFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "InFile({} numbers in {})", self.numbers, self.path.display())
    }
}

/// The same numbers: the name is a hash of them.
impl PartialEq for InFile {
    fn eq(&self, other: &InFile) -> bool {
        self.numbers == other.numbers && self.path.file_name() == other.path.file_name()
    }
}

/// Whether `name` is the whole name of a file this code writes: the finished kind (`sound-likeness-` or
/// `sound-prints-` + 16 hex digits + `.rows`), or a half-written one ([`is_half_kept_name`]).
pub fn is_kept_name(name: &str) -> bool {
    hash_and_end(name).is_some_and(|end| end.is_empty()) || is_half_kept_name(name)
}

/// Whether `name` is the whole name of a file this code is still writing: a finished file's name + `.` + the
/// writer's process number + `-` + its count + `.mo-part`.
pub fn is_half_kept_name(name: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit());
    hash_and_end(name)
        .and_then(|end| end.strip_prefix('.'))
        .and_then(|end| end.strip_suffix(PART))
        .and_then(|writer| writer.split_once('-'))
        .is_some_and(|(process, count)| digits(process) && digits(count))
}

/// What follows a finished file's name in `name`, if it starts with one (of either kind).
fn hash_and_end(name: &str) -> Option<&str> {
    let rest = STARTS.iter().find_map(|start| name.strip_prefix(start))?;
    let (hash, end) = rest.split_at_checked(16)?;
    hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)).then_some(())?;
    end.strip_prefix(END)
}

/// The header of a file of `numbers` numbers in rows of `dim`.
fn header<T: Number>(dim: usize, numbers: usize) -> Vec<u8> {
    let mut h = vec![0u8; HEADER];
    h[..8].copy_from_slice(T::MAGIC);
    h[8..16].copy_from_slice(&(dim as u64).to_le_bytes());
    h[16..24].copy_from_slice(&(numbers as u64).to_le_bytes());
    h
}

/// Keeps the numbers in `parts` (rows of `dim`, as an index holds them) in a file of their kind in `folder`, and
/// opens it. The caller then lets go of `parts`. Says why when it can't (the index stays as it was): no room, a
/// folder that can't be written, a file that won't check.
pub(super) fn keep<T: Number>(dim: usize, parts: &[Box<[T]>], folder: &Path) -> Result<(Arc<InFile>, Kept), String> {
    if cfg!(target_endian = "big") {
        return Err("this computer keeps numbers the other way round from the file".into());
    }
    let numbers: usize = parts.iter().map(|p| p.len()).sum();
    if numbers == 0 {
        return Err("there are none".into());
    }
    let header = header::<T>(dim, numbers);
    let mut hash = Sha256::new();
    hash.update(&header);
    for part in parts {
        hash.update(bytemuck::cast_slice::<T, u8>(part));
    }
    let hash = hash.finalize();
    let hash: String = hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let name: String = format!("{}{hash}{END}", T::START);
    let path = folder.join(&name);
    let bytes = (HEADER + numbers * size_of::<T>()) as u64;
    // The files of other numbers of this kind go first, so they don't take the room this one needs.
    let removed = sweep(folder, &name, T::START);

    let mut written = false;
    if !same(&path, &header, parts) {
        if fs::symlink_metadata(&path).is_ok() {
            // Of this name, yet not these numbers: damaged. It makes way (if it can't, nothing is kept).
            remove_plain(&path).map_err(|e| format!("a damaged file of its name couldn't be removed ({e})"))?;
        }
        write(folder, &path, &name, &header, parts, bytes)?;
        written = true;
    }
    // Checked just now, or inside `write`: a plain file of exactly these numbers.
    let file = File::open(&path).map_err(|e| format!("the file couldn't be opened ({e})"))?;
    let kept = InFile { file, path: path.clone(), numbers, size: size_of::<T>(), told: AtomicBool::new(false) };
    Ok((Arc::new(kept), Kept { file: path, bytes, written, removed }))
}

/// Writes the file under a name of this writer's own, checks it, and gives it its name.
fn write<T: Number>(
    folder: &Path,
    file: &Path,
    name: &str,
    header: &[u8],
    parts: &[Box<[T]>],
    bytes: u64,
) -> Result<(), String> {
    match fs4::available_space(folder) {
        Ok(free) if free < bytes.saturating_add(SPARE.min(bytes)) => {
            return Err(format!(
                "the drive has {} MB free, and the file needs {} MB with {} MB left over",
                free >> 20,
                bytes >> 20,
                SPARE.min(bytes) >> 20
            ));
        }
        _ => {}
    }
    // A name no other writer has, in this process or another.
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let count = COUNT.fetch_add(1, Ordering::Relaxed);
    let part = folder.join(format!("{name}.{}-{count}{PART}", std::process::id()));
    let made = (|| -> std::io::Result<()> {
        let mut out = fs::OpenOptions::new().write(true).create_new(true).open(&part)?;
        out.write_all(header)?;
        for p in parts {
            out.write_all(bytemuck::cast_slice::<T, u8>(p))?;
        }
        out.sync_all()
    })();
    if let Err(e) = made {
        let _ = remove_plain(&part);
        return Err(format!("the file couldn't be written ({e})"));
    }
    if !same(&part, header, parts) {
        let _ = remove_plain(&part);
        return Err("the file written didn't read back the same".into());
    }
    if let Err(e) = fs::rename(&part, file) {
        let _ = remove_plain(&part);
        // Another copy of the app may have finished the same file meanwhile: then that one is used.
        if !same(file, header, parts) {
            return Err(format!("the file couldn't be given its name ({e})"));
        }
    }
    Ok(())
}

/// Whether `file` is a plain file holding exactly `header` and then the numbers in `parts`. Read a piece at a time.
fn same<T: Number>(file: &Path, header: &[u8], parts: &[Box<[T]>]) -> bool {
    let numbers: usize = parts.iter().map(|p| p.len()).sum();
    let Ok(meta) = fs::symlink_metadata(file) else { return false };
    if !meta.is_file() || meta.len() != (header.len() + numbers * size_of::<T>()) as u64 {
        return false;
    }
    let Ok(mut f) = File::open(file) else { return false };
    let mut piece = vec![0u8; PIECE];
    let mut is = |want: &[u8]| {
        want.chunks(PIECE).all(|want| {
            let got = &mut piece[..want.len()];
            f.read_exact(got).is_ok() && got == want
        })
    };
    is(header) && parts.iter().all(|p| is(bytemuck::cast_slice::<T, u8>(p)))
}

/// Removes `path` if it's a plain file (never a folder, never a link).
fn remove_plain(path: &Path) -> std::io::Result<()> {
    if fs::symlink_metadata(path)?.is_file() {
        fs::remove_file(path)
    } else {
        Err(std::io::Error::other("it isn't a plain file"))
    }
}

/// Removes the other files of one kind (their names begin with `start`) in `folder` (not `keep`): finished ones of
/// other numbers, and half-written ones left behind over an hour ago. How many went. One that can't be removed now
/// stays until next time.
fn sweep(folder: &Path, keep: &str, start: &str) -> usize {
    let Ok(entries) = fs::read_dir(folder) else { return 0 };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == keep || !name.starts_with(start) || !is_kept_name(name) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if !meta.is_file() {
            continue;
        }
        if name.ends_with(PART) {
            let age = meta.modified().ok().and_then(|at| now.duration_since(at).ok());
            if age.is_none_or(|age| age < LEFT_BEHIND) {
                continue;
            }
        }
        if fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Removes every file of CLAP's numbers in `folder`: for a library whose index keeps none (CLAP not heard enough, or
/// no library). How many went.
pub fn remove_all(folder: &Path) -> usize {
    sweep(folder, "", <u16 as Number>::START)
}

/// Removes every file of the analyser's fingerprints in `folder`: for a library with none. How many went.
pub fn remove_all_prints(folder: &Path) -> usize {
    sweep(folder, "", <f32 as Number>::START)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whole_names_of_this_kind_are_ours() {
        for ours in [
            "sound-likeness-0123456789abcdef.rows",
            "sound-likeness-0123456789abcdef.rows.4242-0.mo-part",
            "sound-prints-0123456789abcdef.rows",
            "sound-prints-0123456789abcdef.rows.4242-0.mo-part",
        ] {
            assert!(is_kept_name(ours), "{ours}");
        }
        assert!(is_half_kept_name("sound-likeness-0123456789abcdef.rows.4242-17.mo-part"));
        assert!(is_half_kept_name("sound-prints-0123456789abcdef.rows.4242-17.mo-part"));
        assert!(!is_half_kept_name("sound-likeness-0123456789abcdef.rows"));
        assert!(!is_half_kept_name("sound-prints-0123456789abcdef.rows"));
        for not in [
            "",
            "sound-likeness-.rows",
            "sound-prints-.rows",
            "sound-prints-0123456789abcdef",
            "sound-prints0123456789abcdef.rows",
            "sound-printed-0123456789abcdef.rows",
            "sound-likeness-0123456789abcdef",
            "sound-likeness-0123456789abcdef.rows.bak",
            "sound-likeness-0123456789ABCDEF.rows",
            "sound-likeness-0123456789abcdeg.rows",
            "sound-likeness-0123456789abcdef0.rows",
            "sound-likeness-0123456789abcdef.rows.mo-part",
            "sound-likeness-0123456789abcdef.rows.1.mo-part",
            "sound-likeness-0123456789abcdef.rows.-1.mo-part",
            "sound-likeness-0123456789abcdef.rows.1-.mo-part",
            "sound-likeness-0123456789abcdef.rows.x1-2.mo-part",
            "sound-likeness-0123456789abcdef.rows.1-2.part",
            "my sound-likeness-0123456789abcdef.rows",
            "sound-likeness-0123456789abcdef.rows.1-2.mo-part.txt",
            "sound-likeness-é123456789abcdef.rows",
        ] {
            assert!(!is_kept_name(not), "{not}");
        }
    }
}
