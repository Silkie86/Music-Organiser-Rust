//! Bitwig's copies of Library multisamples inside a project. When a project uses a multisample, Bitwig keeps a copy in
//! the project's `multi-samples` folder, adding "-2" or "-3" to the name when it's taken ("Kicks 1 Select-3-2-2-3-2-2-
//! 2-2-2.multisample"), so the same sound sits in many projects. The project points at its own copy, so the copy can't
//! simply go: a "(fixed)" project that points at the Library file is written first (by the converter), and only the
//! copies it changed are offered for the Recycle Bin.
//!
//! This finds the copies that are the same as a Library file byte for byte. Nothing here changes anything.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use rayon::prelude::*;

use crate::size::{Stopped, file_hash, is_link, walk};
use crate::{real_path, text};

/// The folder in a Bitwig project (or template) its multisample copies are kept in.
pub const PROJECT_FOLDER: &str = "multi-samples";

/// A project's copy that's the same as a Library file. It was the same byte for byte when looked at: whatever acts on
/// it (the converter, then the Recycle Bin) compares the two again first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryCopy {
    /// The copy in the project's `multi-samples` folder.
    pub in_project: String,
    /// The Library file it's the same as, byte for byte.
    pub library_file: String,
    /// Its size: the room it takes in the project.
    pub bytes: u64,
}

/// The Library's multisamples, by size, with each one's fingerprint kept once read. Made once and used for every
/// project, so a Library file is read once, and again only if it changed since (rule 12).
pub struct LibraryIndex {
    by_size: HashMap<u64, Vec<PathBuf>>,
    known: Mutex<HashMap<PathBuf, Known>>,
}

/// A Library file's fingerprint, with the size and time it had when it was read: used again only while they still
/// match.
#[derive(Clone, Copy)]
struct Known {
    len: u64,
    modified: Option<SystemTime>,
    print: [u8; 32],
}

impl LibraryIndex {
    /// Lists the multisamples in the Library folders (Bitwig's Multisamples folder, and any other the app knows). A
    /// project's or template's own copies (anything in a `multi-samples` folder, also when reached through a link)
    /// never count as Library files, even when a folder given holds projects: a copy would be pointed at another
    /// project's copy, or at itself, and then binned.
    pub fn new(folders: &[PathBuf], stop: &AtomicBool) -> Result<Self, Stopped> {
        let mut listed: Vec<(u64, PathBuf)> = Vec::new();
        for folder in folders {
            listed.extend(walk(folder, false, stop, |path, meta| {
                (is_multisample(path) && meta.is_file() && !in_a_project(path))
                    .then(|| (meta.len(), path.to_path_buf()))
            })?);
        }
        // Where each really is, worked out once here rather than for every copy.
        listed.par_sort_unstable();
        listed.dedup();
        let kept: Vec<(u64, PathBuf)> = listed
            .into_par_iter()
            .filter(|(_, path)| real_path(&text(path)).is_some_and(|real| !in_a_project(Path::new(&real))))
            .collect();
        if stop.load(Ordering::Relaxed) {
            return Err(Stopped);
        }
        let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
        for (size, path) in kept {
            by_size.entry(size).or_default().push(path);
        }
        Ok(LibraryIndex { by_size, known: Mutex::new(HashMap::new()) })
    }

    /// The copies in one project folder that are the same as a Library file. A copy that's a link, or that isn't the
    /// same as any, isn't listed. Where more than one Library file is the same, the one with the copy's own name (or
    /// that name with fewer of Bitwig's "-2" endings) is picked, or else the shortest name. Only copies and Library
    /// files of the same size are read, each once. Hand the whole list to the converter in one go: each fix writes a
    /// new "(fixed N)" project, and an earlier one still names the copies it didn't cover, so those would have to stay.
    pub fn copies_in(&self, project: &Path, stop: &AtomicBool) -> Result<Vec<LibraryCopy>, Stopped> {
        let folder = project.join(PROJECT_FOLDER);
        if is_link(&folder) || !folder.is_dir() {
            return Ok(Vec::new());
        }
        let found = walk(&folder, false, stop, |path, meta| {
            (is_multisample(path) && meta.is_file() && self.by_size.contains_key(&meta.len()))
                .then(|| (path.to_path_buf(), meta.len()))
        })?;
        // The Library files that could match, each read once (all at the same time), before any copy is compared.
        let mut wanted: Vec<&PathBuf> =
            found.iter().filter_map(|(_, bytes)| self.by_size.get(bytes)).flatten().collect();
        wanted.sort_unstable();
        wanted.dedup();
        let prints: HashMap<&PathBuf, [u8; 32]> =
            wanted.into_par_iter().filter_map(|l| self.print_of(l, stop).map(|print| (l, print))).collect();
        let copies: Vec<Option<LibraryCopy>> = found
            .par_iter()
            .map(|(path, bytes)| {
                let print = file_hash(path, stop)?;
                let same: Vec<&PathBuf> =
                    self.by_size.get(bytes)?.iter().filter(|l| prints.get(l) == Some(&print)).collect();
                let library = best(path, &same)?;
                Some(LibraryCopy { in_project: text(path), library_file: text(library), bytes: *bytes })
            })
            .collect();
        if stop.load(Ordering::Relaxed) {
            return Err(Stopped);
        }
        let mut copies: Vec<LibraryCopy> = copies.into_iter().flatten().collect();
        copies.sort_by(|a, b| a.in_project.cmp(&b.in_project));
        Ok(copies)
    }

    /// A Library file's fingerprint: the one kept while its size and time are as they were, or read now. One that
    /// can't be read now isn't kept (Bitwig may have it open): it's tried again next time.
    fn print_of(&self, path: &Path, stop: &AtomicBool) -> Option<[u8; 32]> {
        let meta = fs::metadata(path).ok()?;
        let (len, modified) = (meta.len(), meta.modified().ok());
        let kept = self.known.lock().ok()?.get(path).copied();
        if let Some(k) = kept.filter(|k| k.len == len && k.modified == modified) {
            return Some(k.print);
        }
        // Changed between the look above and the read: it's kept with the older size and time, so it's read again.
        let print = file_hash(path, stop)?;
        self.known.lock().ok()?.insert(path.to_path_buf(), Known { len, modified, print });
        Some(print)
    }
}

/// A Bitwig multisample file.
fn is_multisample(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("multisample"))
}

/// Whether a path is in a project's or template's own `multi-samples` folder.
fn in_a_project(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case(PROJECT_FOLDER))
}

/// The names a copy may have had in the Library: its own (without the extension) first, then with Bitwig's "-2", "-3"
/// endings (added when a name is taken) taken off one at a time. An ending is 2 to 99, without a leading 0: "Kick-1",
/// "Snare-01" and "Kick-808" end in the name's own number.
fn names(path: &Path) -> Vec<String> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut names = vec![stem.clone()];
    let mut name = stem.as_str();
    while let Some((before, number)) = name.rsplit_once('-') {
        let ending = number.parse::<u8>().is_ok_and(|n| (2..=99).contains(&n)) && !number.starts_with(['0', '+']);
        if !ending || before.is_empty() {
            break;
        }
        name = before;
        names.push(name.to_owned());
    }
    names
}

/// A file's name with all of Bitwig's "-2" endings taken off: "Kicks 1 Select-3-2-2-3-2-2-2-2-2" is "Kicks 1 Select".
pub fn first_name(path: &Path) -> String {
    names(path).pop().unwrap_or_default()
}

/// Of the Library files that are the same as a copy, the one it was most likely copied from: the one with its own
/// name, or that name with the fewest endings taken off, or else the shortest name.
fn best<'a>(copy: &Path, same: &[&'a PathBuf]) -> Option<&'a PathBuf> {
    let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    for name in names(copy) {
        let name = name.to_lowercase();
        if let Some(found) = same.iter().find(|p| stem(p) == name) {
            return Some(found);
        }
    }
    let name_length = |p: &&&PathBuf| (p.file_name().map_or(0, |n| n.to_string_lossy().chars().count()), text(p));
    same.iter().min_by_key(name_length).copied()
}

#[cfg(test)]
mod tests {
    use super::first_name;
    use std::path::Path;

    #[test]
    fn bitwigs_endings_come_off_the_name() {
        let name = |p: &str| first_name(Path::new(p));
        assert_eq!(name("Kicks 1 Select-3-2-2-3-2-2-2-2-2.multisample"), "Kicks 1 Select");
        assert_eq!(name("OHH 1 Select-2-2-2-3-2-2-2.multisample"), "OHH 1 Select");
        assert_eq!(name("Kicks 1 Select.multisample"), "Kicks 1 Select");
        // A name's own dash stays.
        assert_eq!(name("High - BuzzSweep Loop.multisample"), "High - BuzzSweep Loop");
        assert_eq!(name("Stylus Drums - Snare 2.multisample"), "Stylus Drums - Snare 2");
        assert_eq!(name("Kick-808.multisample"), "Kick-808");
        assert_eq!(name("-2.multisample"), "-2");
        // Bitwig's endings start at 2 and have no leading 0.
        assert_eq!(name("Snare-01.multisample"), "Snare-01");
        assert_eq!(name("Kick-1-2.multisample"), "Kick-1");
        assert_eq!(name("Pad-10-2.multisample"), "Pad");
    }
}
