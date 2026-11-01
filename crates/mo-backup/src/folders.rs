//! Backing up by folder (Solomon, 1 Nov 2026: "should be able to bulk back up by folder"): a list of folders you pick
//! (your FL Studio Data, Samples, Bitwig folders...), backed up together in one go. Each folder is either **whole**
//! (everything in it goes, whatever the level says) or **slimmed** (the level's rules: big unused audio and extra big
//! saves stay out).
//!
//! [`gather`] walks every folder in the list (several at a time) and gives what [`crate::plan::plan`] needs: the files,
//! and the whole folders as its "always" folders. A folder inside another one in the list is walked once, as part of
//! the outer one, and still keeps its own choice. A folder that isn't there (a drive not plugged in) is listed as
//! missing, and the rest go on. Folders are only read.

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
    /// The whole folders: pass as [`crate::plan::Folders::always`].
    pub always: Vec<PathBuf>,
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

/// Walks every folder in the list. Call it inside the app's low-priority pool, never on the window's thread.
pub fn gather(list: &[Folder], stop: &AtomicBool) -> Result<Gathered, Stopped> {
    let mut out = Gathered::default();
    let mut walk: Vec<&Path> = Vec::new();
    for (i, f) in list.iter().enumerate() {
        if !f.path.is_dir() {
            out.missing.push(f.path.clone());
            continue;
        }
        if f.mode == Mode::Whole {
            out.always.push(f.path.clone());
        }
        // Walked as part of another folder in the list (the first of two the same is walked).
        let covered = list.iter().enumerate().any(|(j, o)| {
            j != i && o.path.is_dir() && within(&f.path, &o.path) && (!within(&o.path, &f.path) || j < i)
        });
        if !covered {
            walk.push(&f.path);
        }
    }
    let found: Vec<Vec<FileEntry>> = walk.par_iter().map(|p| files(p, stop)).collect::<Result<_, _>>()?;
    out.files = found.into_iter().flatten().collect();
    Ok(out)
}
