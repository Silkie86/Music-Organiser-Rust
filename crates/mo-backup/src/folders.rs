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

use mo_space::size::{FileEntry, Stopped, Walked, walked};
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
    /// Every file in the folders that takes room here, each once.
    pub files: Vec<FileEntry>,
    /// The OneDrive files in them that are only online, named as `files` are: they take no room here and aren't backed
    /// up, but they're there (a song's folder holding another song's save that's only online isn't the song's own).
    pub online_only: Vec<PathBuf>,
    /// Folders in the list that aren't there now.
    pub missing: Vec<PathBuf>,
    /// The listed folders that name their files: each as listed, and where it really is.
    named: Vec<(PathBuf, PathBuf)>,
    /// Where the backup really is: nothing in it is kept.
    backup: Option<PathBuf>,
}

impl Gathered {
    /// The files found, by the folder they're in ([`InFolders`]). Puts them in order once: call it inside the app's
    /// low-priority pool, never on the window's thread.
    pub fn in_folders(&self) -> InFolders<'_> {
        fn in_order(paths: &[&Path]) -> Vec<usize> {
            let mut order: Vec<usize> = (0..paths.len()).collect();
            order.par_sort_unstable_by(|&a, &b| bytes(paths[a]).cmp(bytes(paths[b])));
            order
        }
        let files: Vec<&Path> = self.files.iter().map(|f| f.path.as_path()).collect();
        let online_only: Vec<&Path> = self.online_only.iter().map(PathBuf::as_path).collect();
        InFolders { gathered: self, files: in_order(&files), online_only: in_order(&online_only) }
    }
}

/// The files [`gather`] found, by the folder they're in, so what's in a folder in the list is known without walking it
/// again: the backup's plan measures each song's folder from them (the whole-app review of 19 Nov, SB-3: it walked
/// every song's folder a second time).
pub struct InFolders<'a> {
    gathered: &'a Gathered,
    /// Where each of `gathered.files` is, in the order of their paths' bytes: the files in a folder are side by side.
    files: Vec<usize>,
    /// The same for `gathered.online_only`.
    online_only: Vec<usize>,
}

impl InFolders<'_> {
    /// What a walk of `folder` and its folders would find ([`walked`]), named as `folder` is, when the walk that
    /// gathered the files went through all of it the same way: it's in a folder in the list (by where each really is),
    /// no other folder in the list is inside it, where it really is or by its name (that one names its own files, and
    /// one reached through a link in it isn't in a walk of it), and the backup isn't in it, nor it in the backup.
    /// Nothing when that can't be told, nor when nothing was found in it: a walk of its own says for sure then, and
    /// costs little. What's given stops at the first file `ends` says ends it (given too), as a walk would: a song's
    /// folder that holds another song's save isn't the song's own, however much else is in it (FL Studio songs often
    /// share a folder). Its own files come first, then those in its folders, each in the order of their names: a walk
    /// goes through a folder's own files before its folders too, so in a folder songs share, another song's save is
    /// met before the sounds in its folders are gone through.
    pub fn walked(&self, folder: &str, ends: &dyn Fn(&Path) -> bool) -> Option<Walked> {
        let real = fs::canonicalize(folder).ok()?;
        let named = &self.gathered.named;
        let (listed, at) =
            named.iter().filter(|(_, r)| real.starts_with(r)).max_by_key(|(_, r)| r.components().count())?;
        if self.gathered.backup.as_ref().is_some_and(|b| b.starts_with(&real) || real.starts_with(b)) {
            return None;
        }
        // Named as the gathering walk named it: from the folder in the list it's in.
        let rest = real.strip_prefix(at).ok()?;
        let spelled = if rest.as_os_str().is_empty() { listed.clone() } else { listed.join(rest) };
        if named.iter().any(|(l, r)| r != at && (r.starts_with(&real) || l.starts_with(&spelled))) {
            return None;
        }
        // What's in it starts with its name and the separator a file's name would follow.
        let child = spelled.join("x");
        let prefix = bytes(&child).split_last().map_or(&[][..], |(_, before)| before);
        let named_as = |path: &Path| Some(Path::new(folder).join(path.strip_prefix(&spelled).ok()?));
        // A file of its own: no separator after the folder's name.
        let own = |path: &Path| {
            let rest = bytes(path).get(prefix.len()..).unwrap_or_default();
            !rest.iter().any(|&b| b == b'/' || (cfg!(windows) && b == b'\\'))
        };
        let mut met = Walked::default();
        for its_own in [true, false] {
            for &i in starting_with(&self.online_only, |i| &self.gathered.online_only[i], prefix) {
                let found = &self.gathered.online_only[i];
                if own(found) != its_own {
                    continue;
                }
                let path = named_as(found)?;
                let end = ends(&path);
                met.online_only.push(path);
                if end {
                    return Some(met);
                }
            }
            for &i in starting_with(&self.files, |i| &self.gathered.files[i].path, prefix) {
                let f = &self.gathered.files[i];
                if own(&f.path) != its_own {
                    continue;
                }
                let path = named_as(&f.path)?;
                let end = ends(&path);
                met.files.push(FileEntry { path, size: f.size, modified: f.modified });
                if end {
                    return Some(met);
                }
            }
        }
        (!met.files.is_empty() || !met.online_only.is_empty()).then_some(met)
    }
}

/// A path's bytes, as the system keeps them.
fn bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

/// The places in `order` (paths in the order of their bytes, by `path`) of the paths that start with `prefix`.
fn starting_with<'o, 'p>(order: &'o [usize], path: impl Fn(usize) -> &'p PathBuf, prefix: &[u8]) -> &'o [usize] {
    let from = order.partition_point(|&i| bytes(path(i)) < prefix);
    let to = from + order[from..].partition_point(|&i| bytes(path(i)).starts_with(prefix));
    &order[from..to]
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
    let found: Vec<Walked> = roots
        .par_iter()
        .map(|&root| {
            let inside: Vec<&There<'_>> = there.iter().filter(|t| t.real.starts_with(root)).collect();
            // Left out when it's in the backup; else named from the nearest listed folder it's in.
            let name = |path: &mut PathBuf| {
                if backup.as_ref().is_some_and(|b| path.starts_with(b)) {
                    return false;
                }
                if let Some((t, rest)) = inside.iter().find_map(|t| Some((t, path.strip_prefix(&t.real).ok()?))) {
                    *path = t.listed.join(rest);
                }
                true
            };
            let mut found = walked(root, stop)?;
            found.files.retain_mut(|e| name(&mut e.path));
            found.online_only.retain_mut(name);
            Ok(found)
        })
        .collect::<Result<_, _>>()?;
    for f in found {
        out.files.extend(f.files);
        out.online_only.extend(f.online_only);
    }
    out.named = there.into_iter().map(|t| (t.listed.to_path_buf(), t.real)).collect();
    out.backup = backup;
    Ok(out)
}
