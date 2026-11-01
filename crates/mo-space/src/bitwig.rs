//! Bitwig's copies of Library multisamples inside a project. When a project uses a multisample, Bitwig keeps a copy in
//! the project's `multi-samples` folder, adding "-2" or "-3" to the name when it's taken ("Kicks 1 Select-3-2-2-3-2-2-
//! 2-2-2.multisample"), so the same sound sits in many projects. The project points at its own copy, so the copy can't
//! simply go: a "(fixed)" project that points at the Library file is written first (by the converter), and only the
//! copies it changed are offered for the Recycle Bin.
//!
//! [`LibraryIndex::copies_in`] finds the copies that are the same as a Library file byte for byte, and changes nothing.
//! [`remove_copies`] sends the copies a "(fixed)" project no longer uses to the Recycle Bin, once you've opened it.

use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use rayon::prelude::*;

use crate::clean::{Bin, Done, Problem, STOPPED, Sending, Sent, What, cant_keep};
use crate::size::{Stopped, file_hash, is_link, walk};
use crate::{real_path, really, text};

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
    prints: Prints,
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
            // A Library folder moved to another drive with a link in its place (as Space's moves leave one) is read
            // where it really is, and its files keep the paths you know, through the link: they stay right if the
            // folder is moved back.
            let real = really(folder);
            listed.extend(walk(&real, false, stop, |path, meta| {
                (is_multisample(path) && meta.is_file() && !in_a_project(path)).then(|| {
                    (meta.len(), path.strip_prefix(&real).map_or_else(|_| path.to_path_buf(), |r| folder.join(r)))
                })
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
        Ok(LibraryIndex { by_size, prints: Prints::default() })
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
            wanted.into_par_iter().filter_map(|l| self.prints.of(l, stop).map(|print| (l, print))).collect();
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
}

/// Fingerprints of files once read, used again only while each file's size and time are as they were.
#[derive(Default)]
struct Prints {
    known: Mutex<HashMap<PathBuf, Known>>,
}

impl Prints {
    /// A file's fingerprint: the one kept while its size and time are as they were, or read now. One that can't be
    /// read now isn't kept (Bitwig may have it open): it's tried again next time.
    fn of(&self, path: &Path, stop: &AtomicBool) -> Option<[u8; 32]> {
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

/// A "(fixed)" project the converter wrote, and what it changed.
pub struct FixedProject<'a> {
    /// The "(fixed)" project, beside the original in the project folder.
    pub file: &'a Path,
    /// What it changed: each copy in the project's `multi-samples` folder, and the Library file it uses instead.
    pub changed: &'a [(PathBuf, PathBuf)],
    /// Bitwig's Library folders: the file used instead must be in one of them.
    pub library_folders: &'a [PathBuf],
    /// Why a copy (and the Library file used instead) is still named by a Bitwig file in the project folder or below
    /// it (the fixed project, other saves, autosaves, templates), apart from the original project; `None` when nothing
    /// else names it. The app passes the converter's own check (`mo_convert::relink::still_named`), so it's read again
    /// just before the copy goes.
    pub still_named: &'a (dyn Fn(&Path, &Path) -> Option<String> + Sync),
}

/// Sends the copies a "(fixed)" project no longer uses to the Recycle Bin. This is the one place Space removes samples,
/// an exception to rule 2 that Solomon chose (31 Oct 2026): a copy goes only while the same sound stays in the
/// Library. The app calls it only after you've opened the fixed project and checked it plays, and says first that
/// the original project can't find these copies once they've gone.
///
/// Each is checked again just before it goes: the fixed project is there and is a "(fixed)" one; the copy is a
/// multisample file (not a link) in that project's own `multi-samples` folder (not a link either), on a drive with a
/// Recycle Bin; the file used instead is in a Library folder, isn't a project's copy and isn't the same file on disk;
/// no other Bitwig file in the project folder names the copy ([`FixedProject::still_named`]); and the two are still
/// the same byte for byte (each Library file read once). Any that Windows guards go in one administrator step at the
/// end, checked again just before it asks.
pub fn remove_copies(fixed: &FixedProject<'_>, bin: &dyn Bin, stop: &AtomicBool) -> Done {
    let mut done = Done::default();
    let prints = Prints::default();
    let libraries: Vec<PathBuf> = fixed.library_folders.iter().filter_map(|f| fs::canonicalize(f).ok()).collect();
    let (prints, libraries) = (&prints, &libraries);
    let mut sending = Sending::new(bin, stop);
    // Each file once, by where it really is (worked out before any goes): the first path sent for it, by that place.
    // One whose first path stayed is still checked by the next.
    let reals: Vec<Option<PathBuf>> = fixed.changed.iter().map(|(copy, _)| fs::canonicalize(copy).ok()).collect();
    let mut sent: HashMap<PathBuf, &Path> = HashMap::new();
    for ((copy, library), real_before) in fixed.changed.iter().zip(reals) {
        if let Some(first) = real_before.as_ref().and_then(|r| sent.get(r)) {
            if *first != copy.as_path() {
                let why = format!("It's the same file as {}, given another way, so it's only sent once.", text(first));
                done.problems.push(Problem { path: text(copy), why });
            }
            continue;
        }
        let check = move || still_a_copy(fixed, libraries, prints, copy, library, stop);
        let checked = match check() {
            Ok(checked) => checked,
            Err(why) => {
                done.problems.push(Problem { path: text(copy), why });
                continue;
            }
        };
        // Where it really is, as a plain path (not Windows' long-path form).
        let real = real_path(&text(copy)).unwrap_or_else(|| text(copy));
        // Noted only once it went (or waits for Windows): one whose first path stayed is still tried by the next.
        match sending.send(&text(copy), real, checked.bytes, What::BitwigCopy, &mut done, move || check().map(|_| ())) {
            Sent::Went | Sent::Waits => {
                sent.extend(real_before.into_iter().chain([checked.real]).map(|r| (r, copy.as_path())));
            }
            Sent::Stays => {}
        }
    }
    sending.finish(&mut done);
    done
}

/// A copy that may go: where it really is, and its size.
struct Checked {
    real: PathBuf,
    bytes: u64,
}

/// Whether a copy may still go (see [`remove_copies`]). The cheap checks come first, so nothing is read for nothing.
fn still_a_copy(
    fixed: &FixedProject<'_>,
    libraries: &[PathBuf],
    prints: &Prints,
    copy: &Path,
    library: &Path,
    stop: &AtomicBool,
) -> Result<Checked, String> {
    let stays = |why: &str| Err(why.to_owned());
    if stop.load(Ordering::Relaxed) {
        return stays(STOPPED);
    }
    let fixed_name = fixed.file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if !fixed_name.contains("(fixed") || !fixed_name.ends_with(')') {
        return stays("The project given isn't a \"(fixed)\" one, so its copies stay.");
    }
    if !fs::metadata(fixed.file).is_ok_and(|m| m.is_file()) {
        return stays("The fixed project isn't there now, so the copies it was made for stay.");
    }
    // What's checked is what goes: a name that can't be written as text could reach the bin as another name.
    let Some(copy_text) = copy.to_str() else {
        return stays("Its name can't be read as text, so it stays.");
    };
    let meta = match fs::symlink_metadata(copy) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => return stays("It isn't there any more."),
        Err(_) => return stays("It can't be looked at now, so it stays."),
    };
    if meta.file_type().is_symlink() || !meta.is_file() || !is_multisample(copy) {
        return stays("It isn't a multisample file of the project's own, so it stays.");
    }
    // In the fixed project's own multi-samples folder, where both really are (letter case as on the drive).
    let folder = fixed.file.parent().map(|p| p.join(PROJECT_FOLDER)).unwrap_or_default();
    let real_folder =
        fs::symlink_metadata(&folder).ok().filter(|m| m.is_dir()).and_then(|_| fs::canonicalize(&folder).ok());
    let real = fs::canonicalize(copy).map_err(|_| "It can't be looked at now, so it stays.".to_owned())?;
    if !real_folder.is_some_and(|f| real.starts_with(f)) {
        return stays("It isn't in the fixed project's own multi-samples folder, so it stays.");
    }
    // As a plain path: Windows' long-path form (`\\?\C:\…`) would read as a server's name.
    let real_text = real_path(copy_text).ok_or_else(|| "It can't be looked at now, so it stays.".to_owned())?;
    if let Some(why) = cant_keep(copy_text, &real_text, stop) {
        return stays(why);
    }
    let Some(real_library) = fs::canonicalize(library).ok().filter(|l| l.is_file()) else {
        return stays("The Library file the fixed project uses isn't there now, so the copy stays.");
    };
    if !libraries.iter().any(|l| real_library.starts_with(l)) {
        return stays("What the fixed project uses instead isn't in the Library, so the copy stays.");
    }
    if in_a_project(&real_library) {
        return stays("What the fixed project uses instead is a project's copy too, so this one stays.");
    }
    let library_meta =
        fs::metadata(&real_library).map_err(|_| "The Library file can't be looked at now.".to_owned())?;
    if same_file(&meta, &library_meta) {
        return stays("It's the Library file itself (a hard link to it), so removing it frees nothing. It stays.");
    }
    if library_meta.len() != meta.len() {
        return stays("It isn't the same as the Library file now, so it stays.");
    }
    if let Some(why) = (fixed.still_named)(copy, library) {
        return Err(why);
    }
    let (copy_print, library_print) = (file_hash(copy, stop), prints.of(&real_library, stop));
    if stop.load(Ordering::Relaxed) {
        return stays(STOPPED);
    }
    match (copy_print, library_print) {
        (Some(a), Some(b)) if a == b => Ok(Checked { real, bytes: meta.len() }),
        (Some(_), Some(_)) => stays("It isn't the same as the Library file now, so it stays."),
        _ => stays("It or the Library file couldn't be read whole to compare them, so it stays."),
    }
}

/// Whether two looks are at one file on disk (hard links to it). Only a Mac or Linux can tell here.
#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Windows' file numbers aren't readable from here yet: a hard link counts as a copy (it goes to the bin, and the
/// Library file stays).
#[cfg(not(unix))]
fn same_file(_a: &fs::Metadata, _b: &fs::Metadata) -> bool {
    false
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
