//! An index's half-precision numbers kept in a file beside the app's data and read as they're needed, not held in
//! the app's own memory (Solomon, 2026-11-25: CLAP's were 465 MB of the app's memory for his half a million sounds).
//!
//! The file is mapped: Windows reads the parts a search touches and keeps them only while it has room, and when it's
//! short it lets go of them without writing them to the page file (they're in the file already). The app and its
//! plugin map the same file, so the two hold it once between them.
//!
//! The file's name says what's in it (a hash of its numbers), so:
//! - a library whose fingerprints changed gets a file of another name, and a file in use is never written to (Windows
//!   wouldn't allow it either, with the plugin mapping it);
//! - a file of the right name is still checked, number for number, against the ones just read from the library,
//!   before it's trusted.
//!
//! It's written whole under another name first (ending `.part`), flushed to the disk, read back and checked, and only then
//! given its name. Files of this kind that aren't the one in use are removed (rule 28): only plain files whose whole
//! name is one this code gives ([`is_kept_name`]), never through a link, and a half-written one only once it's an hour
//! old (another copy of the app, or its plugin, may be writing it now).

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// What every file starts with: which kind of file, and which layout.
const MAGIC: &[u8; 8] = b"MOLIKE1\n";
/// The numbers start here: a whole page in, so they lie as the processor wants them.
const HEADER: usize = 4096;
const START: &str = "sound-likeness-";
const END: &str = ".rows";
const PART: &str = ".part";
/// How much room the drive must have left over after the file is written.
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

/// Numbers in a mapped file ([`keep`]).
pub(super) struct Mapped {
    map: memmap2::Mmap,
}

impl Mapped {
    /// The numbers, as the heap held them: every row one after another.
    pub(super) fn numbers(&self) -> &[u16] {
        // Checked when it was mapped: past the header there's a whole number of them, lying as they must.
        self.map.get(HEADER..).and_then(|bytes| bytemuck::try_cast_slice(bytes).ok()).unwrap_or(&[])
    }
}

impl std::fmt::Debug for Mapped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Mapped({} numbers)", self.numbers().len())
    }
}

impl PartialEq for Mapped {
    fn eq(&self, other: &Mapped) -> bool {
        self.numbers() == other.numbers()
    }
}

/// Whether `name` is the whole name of a file this code writes: the finished kind (`sound-likeness-` + 16 hex digits
/// + `.rows`), or a half-written one (the same + `.` + the writer's number + `.part`).
pub fn is_kept_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(START) else { return false };
    let Some((hash, end)) = rest.split_at_checked(16) else { return false };
    if !hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return false;
    }
    match end.strip_prefix(END) {
        Some("") => true,
        Some(part) => part
            .strip_prefix('.')
            .and_then(|p| p.strip_suffix(PART))
            .is_some_and(|pid| !pid.is_empty() && pid.len() <= 10 && pid.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    }
}

/// The header of a file of `numbers` numbers in rows of `dim`.
fn header(dim: usize, numbers: usize) -> Vec<u8> {
    let mut h = vec![0u8; HEADER];
    h[..8].copy_from_slice(MAGIC);
    h[8..16].copy_from_slice(&(dim as u64).to_le_bytes());
    h[16..24].copy_from_slice(&(numbers as u64).to_le_bytes());
    h
}

/// Keeps the numbers in `parts` (rows of `dim`, as an index holds them at half precision) in a file in `folder`, and
/// maps it. The caller then lets go of `parts`. Says why when it can't (the index stays as it was): no room, a
/// folder that can't be written, a file that won't check.
pub(super) fn keep(dim: usize, parts: &[Box<[u16]>], folder: &Path) -> Result<(Arc<Mapped>, Kept), String> {
    if cfg!(target_endian = "big") {
        return Err("this computer keeps numbers the other way round from the file".into());
    }
    let numbers: usize = parts.iter().map(|p| p.len()).sum();
    if numbers == 0 {
        return Err("there are none".into());
    }
    let header = header(dim, numbers);
    let mut hash = Sha256::new();
    hash.update(&header);
    for part in parts {
        hash.update(bytemuck::cast_slice::<u16, u8>(part));
    }
    let hash = hash.finalize();
    let name: String = format!("{START}{}{END}", hash.iter().take(8).map(|b| format!("{b:02x}")).collect::<String>());
    let file = folder.join(&name);
    let bytes = (HEADER + numbers * 2) as u64;

    let mut written = false;
    if !same(&file, &header, parts) {
        if fs::symlink_metadata(&file).is_ok() {
            // Of this name, yet not these numbers: damaged. It makes way (if it can't, nothing is kept).
            remove_plain(&file).map_err(|e| format!("a damaged file of its name couldn't be removed ({e})"))?;
        }
        write(folder, &file, &name, &header, parts, bytes)?;
        written = true;
    }
    let opened = File::open(&file).map_err(|e| format!("the file couldn't be opened ({e})"))?;
    // SAFETY: the file is one this code finished and checked (`same`, just now or inside `write`), and nothing writes
    // to a finished file again: a change of numbers makes a file of another name, and the sweep only removes files,
    // which leaves a mapping whole (Windows refuses even that while it's mapped). So the bytes mapped don't change
    // under the slices handed out. Another program cutting the file short meanwhile would stop the app, as it would
    // if it damaged any file the app has open; nothing of the app's does.
    #[allow(unsafe_code)]
    let map = unsafe { memmap2::Mmap::map(&opened) }.map_err(|e| format!("the file couldn't be mapped ({e})"))?;
    let mapped = Mapped { map };
    if mapped.map.len() as u64 != bytes || mapped.numbers().len() != numbers {
        return Err("the file mapped isn't the size written".into());
    }
    let removed = sweep(folder, &name);
    Ok((Arc::new(mapped), Kept { file, bytes, written, removed }))
}

/// Writes the file under a name of this writer's own, checks it, and gives it its name.
fn write(
    folder: &Path,
    file: &Path,
    name: &str,
    header: &[u8],
    parts: &[Box<[u16]>],
    bytes: u64,
) -> Result<(), String> {
    match fs4::available_space(folder) {
        Ok(free) if free < bytes.saturating_add(SPARE) => {
            return Err(format!(
                "the drive has {} MB free, and the file needs {} MB with 1 GB left over",
                free >> 20,
                bytes >> 20
            ));
        }
        _ => {}
    }
    let part = folder.join(format!("{name}.{}{PART}", std::process::id()));
    // One of this very name is this writer's own, from an earlier try.
    if fs::symlink_metadata(&part).is_ok() {
        remove_plain(&part).map_err(|e| format!("a half-written file couldn't be removed ({e})"))?;
    }
    let made = (|| -> std::io::Result<()> {
        let mut out = fs::OpenOptions::new().write(true).create_new(true).open(&part)?;
        out.write_all(header)?;
        for p in parts {
            out.write_all(bytemuck::cast_slice::<u16, u8>(p))?;
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
        // The plugin (or another copy of the app) may have finished the same file meanwhile: then that one is used.
        if !same(file, header, parts) {
            return Err(format!("the file couldn't be given its name ({e})"));
        }
    }
    Ok(())
}

/// Whether `file` is a plain file holding exactly `header` and then the numbers in `parts`. Read as a file (not
/// through a mapping), a piece at a time.
fn same(file: &Path, header: &[u8], parts: &[Box<[u16]>]) -> bool {
    let numbers: usize = parts.iter().map(|p| p.len()).sum();
    let Ok(meta) = fs::symlink_metadata(file) else { return false };
    if !meta.is_file() || meta.len() != (header.len() + numbers * 2) as u64 {
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
    is(header) && parts.iter().all(|p| is(bytemuck::cast_slice::<u16, u8>(p)))
}

/// Removes `path` if it's a plain file (never a folder, never a link).
fn remove_plain(path: &Path) -> std::io::Result<()> {
    if fs::symlink_metadata(path)?.is_file() {
        fs::remove_file(path)
    } else {
        Err(std::io::Error::other("it isn't a plain file"))
    }
}

/// Removes the other files of this kind in `folder` (not `keep`): finished ones of other numbers, and half-written
/// ones left behind over an hour ago. How many went. One in use (the plugin's, on Windows) stays until next time.
fn sweep(folder: &Path, keep: &str) -> usize {
    let Ok(entries) = fs::read_dir(folder) else { return 0 };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == keep || !is_kept_name(name) {
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

/// Removes every file of this kind in `folder`: for a library whose index keeps none (CLAP not heard enough, or no
/// library). How many went.
pub fn remove_all(folder: &Path) -> usize {
    sweep(folder, "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whole_names_of_this_kind_are_ours() {
        for ours in ["sound-likeness-0123456789abcdef.rows", "sound-likeness-0123456789abcdef.rows.4242.part"] {
            assert!(is_kept_name(ours), "{ours}");
        }
        for not in [
            "",
            "sound-likeness-.rows",
            "sound-likeness-0123456789abcdef",
            "sound-likeness-0123456789abcdef.rows.bak",
            "sound-likeness-0123456789ABCDEF.rows",
            "sound-likeness-0123456789abcdeg.rows",
            "sound-likeness-0123456789abcdef0.rows",
            "sound-likeness-0123456789abcdef.rows.part",
            "sound-likeness-0123456789abcdef.rows..part",
            "sound-likeness-0123456789abcdef.rows.x1.part",
            "my sound-likeness-0123456789abcdef.rows",
            "sound-likeness-0123456789abcdef.rows.1.part.txt",
            "sound-likeness-é123456789abcdef.rows",
        ] {
            assert!(!is_kept_name(not), "{not}");
        }
    }
}
