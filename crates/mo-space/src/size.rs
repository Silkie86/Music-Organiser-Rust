//! How much room files and folders take, and how to say it ("38.2 GB"). The C# app's `DiskSpace`.
//!
//! Every walk here leaves links to other places (junctions, symbolic links) alone, so nothing is counted twice and a
//! folder moved to another drive isn't counted where its link is. On Windows, a OneDrive file that is only online takes
//! no room here, so the size walks leave it out; [`all_files`] still lists it (only its name is read, so nothing
//! downloads).

use std::fs::{self, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Local, NaiveDateTime};
use mo_formats::paths::extension;
use mo_formats::{case, textscan};
use rayon::prelude::*;
use sha2::{Digest, Sha256};

/// A walk was stopped part way (the stop flag was set): what it found so far is dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stopped;

/// What a file or folder holds: every byte, the bytes in sound files, and how many files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Measured {
    pub bytes: u64,
    pub sound_bytes: u64,
    pub files: u64,
}

impl std::ops::Add for Measured {
    type Output = Measured;
    fn add(self, b: Measured) -> Measured {
        Measured {
            bytes: self.bytes.saturating_add(b.bytes),
            sound_bytes: self.sound_bytes.saturating_add(b.sound_bytes),
            files: self.files.saturating_add(b.files),
        }
    }
}

/// Sample-library formats besides plain audio: Kontakt's (.ncw, .nkx, .nkc, .nkr), UVI's (.ufs), SoundFonts, REX
/// loops, Native Instruments' library files.
const SOUND_EXTENSIONS: [&str; 13] =
    [".ncw", ".nkx", ".nkc", ".nkr", ".ufs", ".sf2", ".sf3", ".rx2", ".rex", ".exs", ".sfz", ".wv", ".nicnt"];

/// Files this big count as sounds whatever they're called (Spectrasonics' STEAM, Toontrack's packs): presets and
/// settings are small.
const BIG_FILE: u64 = 16 << 20;

/// Whether a file is sound content (a sample or a sample library) rather than presets or settings.
pub fn is_sound(path: &str, size: u64) -> bool {
    size >= BIG_FILE
        || textscan::is_audio_path(path)
        || SOUND_EXTENSIONS.iter().any(|e| case::eq_ignore_case(e, extension(path)))
}

/// "38.2 GB", "610 MB", "12 KB": three figures at most, as Explorer shows sizes (1 KB is 1024 bytes).
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if bytes == 1 {
        return "1 byte".to_owned();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let name = UNITS.get(unit).copied().unwrap_or("TB");
    if unit == 0 {
        return format!("{bytes} bytes");
    }
    // As .NET's "0", "0.#" and "0.##": rounded, with no zeros at the end.
    let figures = if value >= 100.0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    };
    // Halves round up, as .NET's formatting rounds (Rust's would round 2.5 to 2).
    let scale = 10f64.powi(figures as i32);
    let text = format!("{:.figures$}", (value * scale).round() / scale);
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.') } else { &text };
    format!("{text} {name}")
}

/// One file met on a walk: where it is, its size and when it was last written (local time).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub size: u64,
    pub modified: NaiveDateTime,
}

/// The local time a file was last written, as the library keeps times.
pub(crate) fn modified_of(meta: &Metadata) -> NaiveDateTime {
    meta.modified().map(|t| DateTime::<Local>::from(t).naive_local()).unwrap_or_default()
}

/// Windows' "offline" and "recall on data access": a OneDrive file that's only online, so it takes no room here and
/// reading it downloads it.
#[cfg(windows)]
pub fn is_online_only(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const OFFLINE: u32 = 0x1000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    meta.file_attributes() & (OFFLINE | RECALL_ON_DATA_ACCESS) != 0
}

/// Only Windows has files that are only online.
#[cfg(not(windows))]
pub fn is_online_only(_meta: &Metadata) -> bool {
    false
}

/// Whether a path is a link to another place (a junction or a symbolic link), even one whose place has gone. A
/// OneDrive file or folder isn't one, though Windows marks it in a similar way.
pub fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// What one folder holds, as a walk reads it: its files (mapped), and the folders in it to read next.
struct Listing<T> {
    found: Vec<T>,
    dirs: Vec<PathBuf>,
}

/// How deep a walk goes: far deeper than any real folder. Deeper folders are left out.
pub(crate) const DEEPEST: usize = 128;

/// Walks a folder and its folders a level at a time on every core of the current rayon pool (call it inside the app's
/// low-priority pool, never on the window's thread). Links aren't followed and folders that can't be read are left
/// out. `online_only` says whether OneDrive files that are only online are met too. `map` turns each file into what
/// the caller keeps; returning `None` leaves it out. The order of what comes back isn't fixed.
pub(crate) fn walk<T: Send>(
    folder: &Path,
    online_only: bool,
    stop: &AtomicBool,
    map: impl Fn(&Path, &Metadata) -> Option<T> + Sync,
) -> Result<Vec<T>, Stopped> {
    let mut found = Vec::new();
    if !fs::symlink_metadata(folder).is_ok_and(|m| m.is_dir()) {
        return Ok(found);
    }
    let mut level = vec![folder.to_path_buf()];
    for _ in 0..DEEPEST {
        if level.is_empty() {
            break;
        }
        if stop.load(Ordering::Relaxed) {
            return Err(Stopped);
        }
        let read: Vec<Listing<T>> = level.par_iter().map(|dir| list(dir, online_only, stop, &map)).collect();
        level = Vec::new();
        for l in read {
            found.extend(l.found);
            level.extend(l.dirs);
        }
    }
    if stop.load(Ordering::Relaxed) { Err(Stopped) } else { Ok(found) }
}

fn list<T>(
    dir: &Path,
    online_only: bool,
    stop: &AtomicBool,
    map: &(impl Fn(&Path, &Metadata) -> Option<T> + Sync),
) -> Listing<T> {
    let mut listing = Listing { found: Vec::new(), dirs: Vec::new() };
    let Ok(entries) = fs::read_dir(dir) else { return listing };
    for (n, entry) in entries.flatten().enumerate() {
        if n % 1024 == 1023 && stop.load(Ordering::Relaxed) {
            break;
        }
        // The entry's own type: a link is a link, wherever it points.
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            listing.dirs.push(path);
            continue;
        }
        // On Windows this comes from the folder listing itself, so a file that's only online isn't opened.
        let Ok(meta) = entry.metadata() else { continue };
        if !online_only && is_online_only(&meta) {
            continue;
        }
        if let Some(t) = map(&path, &meta) {
            listing.found.push(t);
        }
    }
    listing
}

/// Every file in a folder and its folders that takes room here, with its size and modified time.
pub fn files(folder: &Path, stop: &AtomicBool) -> Result<Vec<FileEntry>, Stopped> {
    walk(folder, false, stop, |path, meta| {
        Some(FileEntry { path: path.to_path_buf(), size: meta.len(), modified: modified_of(meta) })
    })
}

/// Every file in a folder and its folders, for what's in it rather than the room it takes: OneDrive files that are
/// only online count too.
pub fn all_files(folder: &Path, stop: &AtomicBool) -> Result<Vec<PathBuf>, Stopped> {
    walk(folder, true, stop, |path, _| Some(path.to_path_buf()))
}

/// Adds up a file, or everything in a folder and its folders. Links to other places aren't followed, so nothing is
/// counted twice; folders it can't read are left out. A link itself measures nothing.
pub fn measure(path: &Path, stop: &AtomicBool) -> Result<Measured, Stopped> {
    let Ok(meta) = fs::symlink_metadata(path) else { return Ok(Measured::default()) };
    if meta.file_type().is_symlink() {
        return Ok(Measured::default());
    }
    if !meta.is_dir() {
        if is_online_only(&meta) {
            return Ok(Measured::default());
        }
        let len = meta.len();
        let sound = if is_sound(&path.to_string_lossy(), len) { len } else { 0 };
        return Ok(Measured { bytes: len, sound_bytes: sound, files: 1 });
    }
    let sizes = walk(path, false, stop, |p, meta| {
        let len = meta.len();
        Some(Measured { bytes: len, sound_bytes: if is_sound(&p.to_string_lossy(), len) { len } else { 0 }, files: 1 })
    })?;
    Ok(sizes.into_iter().fold(Measured::default(), |a, b| a + b))
}

/// A file's SHA-256, read once in large pieces; `None` when it can't be read whole, changed while it was read, or when
/// stopped.
pub fn file_hash(path: &Path, stop: &AtomicBool) -> Option<[u8; 32]> {
    let mut file = fs::File::open(path).ok()?;
    let before = file.metadata().ok()?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut read = 0u64;
    loop {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                hash.update(buf.get(..n).unwrap_or_default());
                read += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    let after = fs::metadata(path).ok()?;
    let same = read == before.len() && after.len() == before.len() && after.modified().ok() == before.modified().ok();
    same.then(|| hash.finalize().into())
}

/// Whether any file in a folder or its folders is a OneDrive file that's only online, which the size walks leave out
/// and copying would download first. Only Windows has them.
pub fn has_online_only(folder: &Path, stop: &AtomicBool) -> Result<bool, Stopped> {
    if !cfg!(windows) {
        return Ok(false);
    }
    let online = walk(folder, true, stop, |_, meta| is_online_only(meta).then_some(()))?;
    Ok(!online.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_explorer_shows_them() {
        assert_eq!(size_text(0), "0 bytes");
        assert_eq!(size_text(1), "1 byte");
        assert_eq!(size_text(999), "999 bytes");
        // 1000 bytes is already shown in KB, as .NET's loop does.
        assert_eq!(size_text(1000), "0.98 KB");
        assert_eq!(size_text(1024), "1 KB");
        assert_eq!(size_text(12 * 1024), "12 KB");
        assert_eq!(size_text(610 << 20), "610 MB");
        assert_eq!(size_text((38.2 * (1u64 << 30) as f64) as u64), "38.2 GB");
        assert_eq!(size_text(1536 << 20), "1.5 GB");
        assert_eq!(size_text(u64::MAX), "16777216 TB");
    }

    #[test]
    fn sounds_are_audio_sample_libraries_or_big_files() {
        assert!(is_sound("C:/Kick.WAV", 10));
        assert!(is_sound("C:/Piano.nkx", 10));
        assert!(is_sound("C:/Pack.steam", BIG_FILE));
        assert!(!is_sound("C:/Preset.fxp", 10));
    }
}
