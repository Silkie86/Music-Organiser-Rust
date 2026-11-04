//! Backing up by folder (Solomon, 1 Nov 2026: "should be able to bulk back up by folder"): a list of folders you pick
//! (your FL Studio Data, Samples, Bitwig folders...), backed up together in one go. Each folder is either **whole**
//! (everything in it goes, whatever the level says) or **slimmed** (the level's rules: big unused audio and extra big
//! saves stay out).
//!
//! [`gather`] walks every folder in the list (several at a time) and gives the files [`crate::plan::plan`] needs; give
//! it the list as [`crate::plan::Listed`], so it knows which files are in a whole folder. A folder inside another one
//! in the list is walked once, as part of the outer one, and still keeps its own choice. A folder that isn't there (a
//! drive not plugged in, a link to nowhere) is listed as missing, and the rest go on. Folders are only read.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_space::size::{FileEntry, Stopped, files};
use rayon::prelude::*;

/// How a folder in the list is backed up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Everything in it goes.
    Whole,
    /// The level's rules decide.
    #[default]
    Slimmed,
}

/// One folder in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub path: PathBuf,
    pub mode: Mode,
}

/// The list as plain text, one folder a line ("whole" or "slimmed", a tab, the path), to keep in the settings.
pub fn to_text(list: &[Folder]) -> String {
    list.iter()
        .map(|f| {
            let mode = match f.mode {
                Mode::Whole => "whole",
                Mode::Slimmed => "slimmed",
            };
            format!("{mode}\t{}\n", f.path.display())
        })
        .collect()
}

/// The list back from [`to_text`]. A line that doesn't make sense is skipped.
pub fn from_text(text: &str) -> Vec<Folder> {
    text.lines()
        .filter_map(|line| {
            let (mode, path) = line.split_once('\t')?;
            let mode = match mode {
                "whole" => Mode::Whole,
                "slimmed" => Mode::Slimmed,
                _ => return None,
            };
            (!path.is_empty()).then(|| Folder { path: PathBuf::from(path), mode })
        })
        .collect()
}

/// What the walk found.
#[derive(Clone, Debug, Default)]
pub struct Gathered {
    /// Every file in the folders, each once.
    pub files: Vec<FileEntry>,
    /// Folders in the list that aren't there now.
    pub missing: Vec<PathBuf>,
}

/// Whether `inner` is `outer` or inside it, letter case aside on Windows and Mac.
pub fn within(inner: &Path, outer: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        let fold = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
        fold(inner).starts_with(fold(outer))
    } else {
        inner.starts_with(outer)
    }
}

/// Walks every folder in the list, leaving out anything in the backup itself. Call it inside the app's low-priority
/// pool, never on the window's thread.
///
/// Folders are matched by where each really is, so links don't fool the walk (Space's folder move leaves a junction
/// where a folder was):
///
/// - A listed folder that is itself a link is walked where it really is. A link that leads nowhere, or whose real
///   place can't be told, is missing: it's never walked as empty.
/// - A listed folder inside another one is walked once, as part of the outer one, even when it's reached through a
///   link (the outer walk leaves links alone, so the inner one is walked on its own then).
/// - Each file takes the spelling of the nearest listed folder it's in (the first of two that are the same place), so
///   it matches your projects' paths and that folder keeps its own choice.
pub fn gather(list: &[Folder], backup: &Path, stop: &AtomicBool) -> Result<Gathered, Stopped> {
    struct There<'a> {
        listed: &'a Path,
        real: PathBuf,
    }
    let mut out = Gathered::default();
    let mut there: Vec<There<'_>> = Vec::new();
    for f in list {
        let real = if f.path.is_dir() { fs::canonicalize(&f.path).ok() } else { None };
        let Some(real) = real else {
            out.missing.push(f.path.clone());
            continue;
        };
        // The first of two listed folders that are the same place names its files.
        if !there.iter().any(|t| t.real == real) {
            there.push(There { listed: &f.path, real });
        }
    }
    // Deepest first, so the first one a file is in is the nearest.
    there.sort_by_key(|t| std::cmp::Reverse(t.real.components().count()));
    let roots: Vec<&Path> = there
        .iter()
        .filter(|t| !there.iter().any(|o| o.real != t.real && t.real.starts_with(&o.real)))
        .map(|t| t.real.as_path())
        .collect();
    let backup = fs::canonicalize(backup).ok();
    let found: Vec<Vec<FileEntry>> = roots
        .par_iter()
        .map(|&root| {
            let inside: Vec<&There<'_>> = there.iter().filter(|t| t.real.starts_with(root)).collect();
            let mut found = files(root, stop)?;
            found.retain_mut(|e| {
                if backup.as_ref().is_some_and(|b| e.path.starts_with(b)) {
                    return false;
                }
                if let Some((t, rest)) = inside.iter().find_map(|t| Some((t, e.path.strip_prefix(&t.real).ok()?))) {
                    e.path = t.listed.join(rest);
                }
                true
            });
            Ok(found)
        })
        .collect::<Result<_, _>>()?;
    out.files = found.into_iter().flatten().collect();
    Ok(out)
}
