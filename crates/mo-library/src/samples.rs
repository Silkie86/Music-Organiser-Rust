//! Finding a project's samples on this computer, as the C# app's `SampleLocator` does. Solomon's rule (2026-10-15,
//! rule 13): "when deciding if something is missing check the project folders and native folders for that daw". So a
//! sample counts as found, in this order:
//!
//! 1. where the project says it is (a relative path from the project's folder);
//! 2. at the same path on the project's drive, for a drive that was given a new letter (F: is now K:);
//! 3. where it would be if the folders it was in moved with the project: a path into the project's own folder, or
//!    into a folder above it ("F:\Bitwig Studio\Projects\Other Song\samples\x.wav" next to
//!    "K:\Bitwig Studio\Projects\This Song");
//! 4. beside the project, or in a folder beside it that a DAW collects into (samples, recordings, Media, Audio);
//! 5. anywhere in the project's folder tree with the same file name (Bitwig's bounce, multi-samples and recordings
//!    folders, FL Studio's Audio and Samples folders);
//! 6. in the DAW's own content folders (its factory packs, user library and installed sound packages), where the
//!    folder it's in must have the same name too, since those folders hold thousands of sounds.
//!
//! Each folder tree is read once by a locator. A scan uses one locator and lets it go when it's done, so the next scan
//! sees samples put back since, and the trees aren't kept between scans. It asks the disk, so never on the window's
//! thread.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use mo_formats::case;
use mo_formats::paths::{extension, file_name};
use mo_formats::textscan::is_audio_path;

/// The most files read from one folder tree: a project kept at the top of a big drive stops there.
pub const MOST_FILES: usize = 200_000;

/// The project's own folder and the folders beside it that DAWs collect samples into.
const BESIDE: [&str; 6] = ["", "samples", "recordings", "Media", "Audio", "audio"];

/// Folders inside a DAW's folders that hold projects, autosaves or history rather than sounds.
const NOT_CONTENT: [&str; 8] =
    ["Projects", "Songs", "Shows", "auto-backups", "Backup", "History", "$RECYCLE.BIN", "System Volume Information"];

/// The most folder trees a locator keeps at once.
const MOST_TREES: usize = 4096;

/// Where the DAW's own content folders are for a project, by its path.
type ContentFolders = dyn Fn(&str) -> Vec<String> + Send + Sync;

/// Finds samples, reading each folder tree once. Shared between the threads of one scan.
pub struct SampleLocator {
    content: Box<ContentFolders>,
    trees: Mutex<HashMap<String, Arc<OnceLock<FolderIndex>>>>,
}

impl Default for SampleLocator {
    fn default() -> SampleLocator {
        SampleLocator::new(Box::new(daw_content_folders))
    }
}

impl SampleLocator {
    /// A locator that looks in `content` for a DAW's own folders (tests give their own).
    pub fn new(content: Box<ContentFolders>) -> SampleLocator {
        SampleLocator { content, trees: Mutex::new(HashMap::new()) }
    }

    /// Whether this sample is on this computer, by the rules above. FL Studio's own content
    /// ("%FLStudioFactoryData%…") always is.
    pub fn found(&self, sample: &str, project: &str) -> bool {
        sample.starts_with('%') || self.locate(sample, project).is_some()
    }

    /// Where this sample is on this computer, by the rules above, or `None` when it's missing.
    pub fn locate(&self, sample: &str, project: &str) -> Option<String> {
        if sample.is_empty() {
            return None;
        }
        if let Some(c) = candidates(sample, project).into_iter().find(|c| Path::new(c).is_file()) {
            return Some(c);
        }
        let (_, parts) = split(sample);
        let name = *parts.last()?;
        let project_dir = Path::new(project).parent().filter(|d| !d.as_os_str().is_empty())?;
        for sub in BESIDE {
            let near = if sub.is_empty() { project_dir.join(name) } else { project_dir.join(sub).join(name) };
            if near.is_file() {
                return near.to_str().map(str::to_owned);
            }
        }
        // A project kept at the top of a drive: only the files beside it, not the whole drive.
        let dir = project_dir.to_str()?;
        let at_top = !split(dir).0.is_empty() && split(dir).1.is_empty();
        if let Some(found) = self.find_in(dir, if at_top { 0 } else { usize::MAX }, name, None) {
            return Some(found);
        }
        let folder = parts.len().checked_sub(2).and_then(|i| parts.get(i)).copied();
        (self.content)(project).iter().find_map(|c| self.find_in(c, usize::MAX, name, folder))
    }

    /// A file called `name` in the tree under `root` (in a folder called `folder`, when given), reading the tree the
    /// first time it's asked for.
    fn find_in(&self, root: &str, depth: usize, name: &str, folder: Option<&str>) -> Option<String> {
        self.tree(root, depth).get()?.find(name, folder)
    }

    fn tree(&self, folder: &str, depth: usize) -> Arc<OnceLock<FolderIndex>> {
        let key = case::key(folder.trim_end_matches(['\\', '/']));
        let slot = {
            let Ok(mut trees) = self.trees.lock() else { return Arc::new(OnceLock::new()) };
            if trees.len() > MOST_TREES {
                trees.retain(|k, _| *k == key);
            }
            trees.entry(key).or_default().clone()
        };
        // Read outside the lock, once: another thread wanting the same tree waits for this read.
        slot.get_or_init(|| FolderIndex::read(folder, depth));
        slot
    }
}

/// The sample files in a folder tree, by file name.
#[derive(Default)]
struct FolderIndex {
    by_name: HashMap<String, Vec<String>>,
}

impl FolderIndex {
    fn read(root: &str, depth: usize) -> FolderIndex {
        let mut index = FolderIndex::default();
        let mut files = 0;
        let mut todo: Vec<(PathBuf, usize)> = vec![(PathBuf::from(root), 0)];
        while let Some((dir, level)) = todo.pop() {
            if files >= MOST_FILES {
                break;
            }
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            let mut dirs = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(p) = path.to_str() else { continue };
                // A link is looked at as what it points to, as Windows' own listing does.
                let is_dir = fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
                if is_dir {
                    let name = file_name(p);
                    if level < depth
                        && !name.starts_with('.')
                        && !NOT_CONTENT.iter().any(|n| case::eq_ignore_case(n, name))
                    {
                        dirs.push(p.to_owned());
                    }
                } else if is_sample(p) && files < MOST_FILES {
                    index.by_name.entry(case::key(file_name(p))).or_default().push(p.to_owned());
                    files += 1;
                }
            }
            // Walked in name order, as the C# app walks them, so the same file wins.
            dirs.sort_by_cached_key(|d| std::cmp::Reverse(case::key(d)));
            // A link back up its own tree would go round for ever: it stops at a depth no real folder reaches.
            if level < 64 {
                todo.extend(dirs.into_iter().map(|d| (PathBuf::from(d), level + 1)));
            }
        }
        index
    }

    /// A file with this name, in a folder with this name when one is given; one that's still there.
    fn find(&self, name: &str, folder: Option<&str>) -> Option<String> {
        self.by_name
            .get(&case::key(name))?
            .iter()
            .find(|f| {
                let parent = Path::new(f.as_str()).parent().and_then(|p| p.to_str()).map(file_name).unwrap_or_default();
                folder.is_none_or(|want| same(parent, want)) && Path::new(f.as_str()).is_file()
            })
            .cloned()
    }
}

fn is_sample(path: &str) -> bool {
    is_audio_path(path) || case::eq_ignore_case(extension(file_name(path)), ".multisample")
}

fn same(a: &str, b: &str) -> bool {
    case::eq_ignore_case(a.trim_end_matches(['\\', '/']), b.trim_end_matches(['\\', '/']))
}

fn is_drive(root: &str) -> bool {
    let b = root.as_bytes();
    b.len() >= 2 && b.first().is_some_and(u8::is_ascii_alphabetic) && b.get(1) == Some(&b':')
}

/// A path's root ("K:\", "/", "\\" for a share, or "" when relative) and its folder and file names.
fn split(path: &str) -> (&str, Vec<&str>) {
    let b = path.as_bytes();
    let root_len = if b.len() >= 3
        && b.first().is_some_and(u8::is_ascii_alphabetic)
        && b.get(1) == Some(&b':')
        && matches!(b.get(2), Some(b'\\' | b'/'))
    {
        3
    } else if path.starts_with(r"\\") {
        2
    } else if path.starts_with(['/', '\\']) {
        1
    } else {
        0
    };
    let (root, rest) = path.split_at(root_len);
    (root, rest.split(['\\', '/']).filter(|p| !p.is_empty()).collect())
}

/// The paths to try for a sample before searching folders, best first: as saved, then moved with a drive or a folder
/// above the project. Works on Windows paths on any system, so it can be tested anywhere.
pub fn candidates(sample: &str, project: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut add = |p: String| {
        if seen.insert(case::key(&p)) {
            out.push(p);
        }
    };
    let (root, parts) = split(sample);
    let (project_root, project_parts) = split(project);
    let dir = project_parts.get(..project_parts.len().saturating_sub(1)).unwrap_or_default();
    let sep = if project.contains('\\') && !project.contains('/') { "\\" } else { std::path::MAIN_SEPARATOR_STR };
    let join = |r: &str, p: &[&str]| format!("{r}{}", p.join(sep));

    if !root.is_empty() {
        add(sample.to_owned());
    } else {
        add(join(project_root, &[dir, &parts].concat()));
    }
    // A drive that was given a new letter: the same path on the project's drive.
    if is_drive(root) && is_drive(project_root) && !same(root, project_root) {
        add(join(project_root, &parts));
    }
    // A folder that moved with the project: the project's own folder first, then each folder above it.
    if root.is_empty() {
        return out;
    }
    for k in (0..dir.len()).rev() {
        let Some(here) = dir.get(k) else { continue };
        let Some(at) = (0..parts.len().saturating_sub(1)).rev().find(|&i| parts.get(i).is_some_and(|p| same(p, here)))
        else {
            continue;
        };
        let (Some(head), Some(tail)) = (dir.get(..=k), parts.get(at + 1..)) else { continue };
        add(join(project_root, &[head, tail].concat()));
    }
    out
}

/// The DAW's own content folders on this computer for a project, by its file type: FL Studio's packs and user data
/// folder, Bitwig's library and installed sound packages, Studio One's user folder. Projects kept in a "Bitwig Studio"
/// folder of their own can have a library beside them.
pub fn daw_content_folders(project: &str) -> Vec<String> {
    let ext = case::to_lower(extension(file_name(project)));
    let mut folders = fixed_content_folders(&ext);
    if ext == ".bwproject" {
        let mut d = Path::new(project).parent();
        while let Some(dir) = d {
            if dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| case::eq_ignore_case(n, "Bitwig Studio")) {
                let library = dir.join("Library");
                if let Some(l) = library.to_str().filter(|_| library.is_dir())
                    && !folders.iter().any(|f| case::eq_ignore_case(f, l))
                {
                    folders.push(l.to_owned());
                }
            }
            d = dir.parent();
        }
    }
    folders
}

fn fixed_content_folders(ext: &str) -> Vec<String> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = env(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    // Windows and a Mac keep the DAWs' user folders in Documents; on Linux they sit in the home folder.
    let documents = home.as_ref().map(|h| if cfg!(target_os = "linux") { h.clone() } else { h.join("Documents") });
    let mut folders: Vec<PathBuf> = Vec::new();
    match ext {
        ".flp" => {
            for programs in [env("ProgramFiles"), env("ProgramFiles(x86)")].into_iter().flatten() {
                let image_line = programs.join("Image-Line");
                for install in fs::read_dir(&image_line).into_iter().flatten().flatten() {
                    if install.file_name().to_str().is_some_and(|n| n.starts_with("FL Studio")) {
                        folders.push(install.path().join("Data").join("Patches").join("Packs"));
                    }
                }
            }
            if let Some(d) = &documents {
                folders.push(d.join("Image-Line").join("FL Studio"));
                folders.push(d.join("Image-Line").join("Data").join("FL Studio"));
            }
        }
        ".bwproject" => {
            if let Some(d) = &documents {
                folders.push(d.join("Bitwig Studio"));
            }
            if cfg!(windows) {
                folders.extend(env("LOCALAPPDATA").map(|l| l.join("Bitwig Studio").join("installed-packages")));
                folders.extend(env("ProgramData").map(|p| p.join("Bitwig Studio").join("installed-packages")));
            } else if cfg!(target_os = "macos") {
                folders.extend(
                    home.as_ref()
                        .map(|h| h.join("Library/Application Support/Bitwig/Bitwig Studio/installed-packages")),
                );
            } else {
                folders.extend(home.as_ref().map(|h| h.join(".BitwigStudio").join("installed-packages")));
            }
        }
        ".song" => {
            if let Some(d) = &documents {
                folders.push(d.join("Studio One"));
                folders.push(d.join("Fender Studio Pro"));
            }
        }
        _ => {}
    }
    let mut out: Vec<String> = Vec::new();
    for f in folders.iter().filter(|f| f.is_absolute() && f.is_dir()) {
        if let Some(s) = f.to_str().filter(|s| !out.iter().any(|o| case::eq_ignore_case(o, s))) {
            out.push(s.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_follow_a_moved_drive_and_folder() {
        let project = r"K:\Bitwig Studio\Projects\This Song\This Song.bwproject";
        assert_eq!(
            candidates(r"F:\Bitwig Studio\Projects\Other Song\samples\x.wav", project),
            [
                r"F:\Bitwig Studio\Projects\Other Song\samples\x.wav",
                r"K:\Bitwig Studio\Projects\Other Song\samples\x.wav"
            ]
        );
        // A relative path is from the project's folder.
        assert_eq!(candidates(r"samples\kick.wav", r"C:\m\Song\Song.flp"), [r"C:\m\Song\samples\kick.wav"]);
        // A folder that moved with the project: the part after the project's own folder name.
        let moved = candidates(r"F:\old\Night Drive\Audio\vox.wav", r"K:\new\Night Drive\Night Drive.flp");
        assert_eq!(moved.last().map(String::as_str), Some(r"K:\new\Night Drive\Audio\vox.wav"));
        assert!(candidates("", r"C:\m\a.flp").len() <= 1);
    }

    #[test]
    fn roots_and_drives() {
        assert_eq!(split(r"K:\a\b.wav"), (r"K:\", vec!["a", "b.wav"]));
        assert_eq!(split(r"\\server\share\b.wav"), (r"\\", vec!["server", "share", "b.wav"]));
        assert_eq!(split("/m/b.wav"), ("/", vec!["m", "b.wav"]));
        assert_eq!(split("b.wav"), ("", vec!["b.wav"]));
        assert!(is_drive("K:") && !is_drive("/") && !is_drive(""));
    }
}
