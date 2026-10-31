//! Finding project files in your folders and working out what the library needs to read again, as the C# app's
//! `Scanner` does: only new and changed files are read (by size and modified time), a DAW whose reader got better has
//! its projects read again, and files that have gone are listed so they can be marked missing.
//!
//! Nothing here reads a project or changes the library: [`find_projects`] lists files, and [`plan`] compares that list
//! with what the library knows. The folders are walked a level at a time with every core of the rayon pool it runs in,
//! so call it inside the app's low-priority pool (rule 21), never on the window's thread.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use mo_formats::case;
use mo_formats::paths::{extension, file_name};
use rayon::prelude::*;

/// The DAW whose reader reads a file, by its ending, as the library names it; `None` for a file no reader knows.
pub fn daw_of(path: &str) -> Option<&'static str> {
    match case::to_lower(extension(file_name(path))).as_str() {
        ".bwproject" => Some("Bitwig Studio"),
        ".flp" => Some("FL Studio"),
        ".song" => Some("Studio One"),
        _ => None,
    }
}

/// DAW autosave folders: Bitwig "auto-backups", FL Studio "Backup", Studio One "History".
const AUTOSAVE_DIRS: [&str; 6] = ["auto-backups", "backup", "backups", "history", "autosave", "autosaves"];

/// Folders that never hold your projects: the recycle bin, Windows' own, a Mac zip's "._" files, code.
const SKIP_DIRS: [&str; 4] = ["$RECYCLE.BIN", "System Volume Information", "__MACOSX", "node_modules"];

/// Demo songs, tutorials and templates that came with the DAW rather than your own projects. Old FL Studio installs
/// keep demos in "Cool stuff" and loop packs in Data\Patches.
const FACTORY_DIRS: [&str; 7] =
    ["Cool stuff", "Demo songs", "Product demos", "Templates", "Template", "Tutorial", "Tutorials"];

fn one_of(name: &str, list: &[&str]) -> bool {
    list.iter().any(|d| case::eq_ignore_case(d, name))
}

/// A DAW's autosave folder.
pub fn is_autosave_dir(name: &str) -> bool {
    one_of(name, &AUTOSAVE_DIRS)
}

/// FL Studio's own backup copies, should they end up outside its Backup folder: "Song (autosave).flp" and
/// "Song (overwritten at 2213h05).flp".
pub fn is_autosave_file(name: &str) -> bool {
    let name = case::to_lower(name);
    name.contains(" (autosave)") || name.contains(" (overwritten at ")
}

/// Whether the scan leaves out a folder called `name` inside one called `parent`.
pub fn skip_dir(name: &str, parent: &str) -> bool {
    name.starts_with('.')
        || one_of(name, &SKIP_DIRS)
        || is_autosave_dir(name)
        || one_of(name, &FACTORY_DIRS)
        || (case::eq_ignore_case(name, "Patches") && case::eq_ignore_case(parent, "Data"))
}

/// A file the library lists: one a reader knows, leaving out a Mac's "._" shadows and DAW autosaves.
pub fn is_project_file(path: &str) -> bool {
    let name = file_name(path);
    daw_of(path).is_some() && !name.starts_with("._") && !is_autosave_file(name)
}

/// A path as the library compares paths: forward slashes, no slash at the end, and ignoring case (the library keeps
/// each path once whatever its case, as Windows does).
fn path_key(path: &str) -> String {
    let key = case::key(&path.replace('\\', "/"));
    match key.trim_end_matches('/') {
        "" => "/".to_owned(),
        trimmed => trimmed.to_owned(),
    }
}

/// [`is_inside`] on paths already made into [`path_key`]s.
fn key_inside(path: &str, folder: &str) -> bool {
    path == folder || path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/') || folder.ends_with('/'))
}

/// Whether `path` is `folder` or inside it, whichever way the slashes lean and ignoring case: "K:\Music Old" isn't
/// inside "K:\Music".
pub fn is_inside(path: &str, folder: &str) -> bool {
    key_inside(&path_key(path), &path_key(folder))
}

/// Whether the scan leaves this file out: an autosave copy, or inside a skipped folder under its root (the innermost
/// root, when one is inside another).
pub fn is_ignored(path: &str, roots: &[String]) -> bool {
    if is_autosave_file(file_name(path)) {
        return true;
    }
    let Some(root) = roots.iter().filter(|r| is_inside(path, r)).max_by_key(|r| parts(r).count()) else {
        return false;
    };
    // The folders between the root and the file, counted by name rather than by letters, so a root written in
    // another case or with other slashes still lines up.
    let folders: Vec<&str> = parts(path).collect();
    let below = folders.get(parts(root).count()..folders.len().saturating_sub(1)).unwrap_or_default();
    let mut parent = parts(root).last().unwrap_or_default();
    for name in below {
        if skip_dir(name, parent) {
            return true;
        }
        parent = name;
    }
    false
}

/// The names in a path, whichever way its slashes lean.
fn parts(path: &str) -> impl Iterator<Item = &str> {
    path.split(['\\', '/']).filter(|d| !d.is_empty())
}

/// Whether a path that's gone was more likely a folder than a file: a file type is a dot then a short run of letters
/// and digits starting with a letter (.wav, .mp3, .asd), so anything else after the last dot is part of a name
/// ("Vol.2", "Drums. Dry").
pub fn looks_like_folder(path: &str) -> bool {
    let ext = extension(file_name(path)).get(1..).unwrap_or_default();
    !(1..=5).contains(&ext.len())
        || !ext.starts_with(|c: char| c.is_ascii_alphabetic())
        || !ext.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A project file found in a folder, with the size and modified time (Unix seconds) the folder listing gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundFile {
    pub path: String,
    pub size: i64,
    pub mtime: i64,
}

/// What [`find_projects`] found.
#[derive(Clone, Debug, Default)]
pub struct Found {
    /// Sorted by path, ignoring case; each file once.
    pub files: Vec<FoundFile>,
    /// Folders that couldn't be read this time (busy, or their permissions changing): their projects stay as they were.
    pub unreadable: Vec<String>,
}

/// How deep a walk goes: far deeper than any real folder of songs. Folders below are counted as unreadable, so the
/// saves in them aren't taken for gone.
const DEEPEST: usize = 64;

/// Every project file under `roots`, a level of folders at a time on every core of the current rayon pool. A root
/// that isn't there (an unplugged drive) is counted as unreadable, so its saves stay as they were; a root inside
/// another is walked once, as part of the outer one.
pub fn find_projects(roots: &[String]) -> Found {
    let mut found = Found::default();
    let keys: Vec<String> = roots.iter().map(|r| path_key(r)).collect();
    let mut level: Vec<String> = Vec::new();
    let mut outer: Vec<String> = Vec::new();
    for (i, (root, key)) in roots.iter().zip(&keys).enumerate() {
        // Inside another root, or the same root again (only the first of those is kept).
        let inner = keys.iter().enumerate().any(|(j, k)| j != i && key_inside(key, k) && (k != key || j < i));
        if inner {
            continue;
        }
        if Path::new(root.as_str()).is_dir() {
            level.push(root.clone());
            outer.push(key.clone());
        } else {
            found.unreadable.push(root.clone());
        }
    }
    // Where each root really is, so a folder link back into one isn't walked a second time.
    let real_roots: Vec<String> =
        level.iter().filter_map(|r| fs::canonicalize(r).ok()?.to_str().map(path_key)).collect();
    let mut linked: HashSet<String> = HashSet::new();
    for _ in 0..DEEPEST {
        if level.is_empty() {
            break;
        }
        let read: Vec<Listing> = level.par_iter().map(|dir| list(dir)).collect();
        level = Vec::new();
        for l in read {
            found.files.extend(l.files);
            level.extend(l.dirs);
            found.unreadable.extend(l.unreadable);
            for (dir, real) in l.links {
                // A link into a root is walked there already, and two links to one folder walk it once.
                if !real_roots.iter().any(|r| key_inside(&real, r)) && linked.insert(real) {
                    level.push(dir);
                }
            }
        }
    }
    found.unreadable.extend(level);
    found.files.sort_by_cached_key(|f| case::key(&f.path));
    found.files.dedup_by(|a, b| case::eq_ignore_case(&a.path, &b.path));
    found
}

/// What one folder holds: its project files, the folders in it to walk next, the folder links in it with where they
/// really go, and what couldn't be read (the folder itself, or an entry in it that couldn't be looked at).
#[derive(Default)]
struct Listing {
    files: Vec<FoundFile>,
    dirs: Vec<String>,
    links: Vec<(String, String)>,
    unreadable: Vec<String>,
}

/// Lists one folder.
fn list(dir: &str) -> Listing {
    let mut out = Listing::default();
    let Ok(entries) = fs::read_dir(dir) else {
        out.unreadable.push(dir.to_owned());
        return out;
    };
    let parent = file_name(dir.trim_end_matches(['\\', '/']));
    // An entry that can't be looked at (a file still being written, a share that hiccups) makes the folder count as
    // unreadable this time, so none of its saves is taken for gone.
    let mut shaky = false;
    for entry in entries {
        let Ok(entry) = entry else {
            shaky = true;
            continue;
        };
        let Some(path) = entry.path().to_str().map(str::to_owned) else { continue };
        let Ok(kind) = entry.file_type() else {
            shaky = true;
            continue;
        };
        if kind.is_symlink() {
            // A link is followed to what it points at, as Windows' own listing does for a folder junction; one that
            // points nowhere is left out.
            let Ok(meta) = fs::metadata(&path) else { continue };
            if meta.is_dir() {
                if !skip_dir(file_name(&path), parent) {
                    if let Some(real) = fs::canonicalize(&path).ok().and_then(|r| r.to_str().map(path_key)) {
                        out.links.push((path, real));
                    }
                }
            } else if is_project_file(&path) {
                out.files.push(FoundFile { size: size(&meta), mtime: mtime(&meta), path });
            }
        } else if kind.is_dir() {
            if !skip_dir(file_name(&path), parent) {
                out.dirs.push(path);
            }
        } else if is_project_file(&path) {
            // On Windows the size and date come with the listing itself, so this doesn't ask the disk again.
            match entry.metadata() {
                Ok(meta) => out.files.push(FoundFile { size: size(&meta), mtime: mtime(&meta), path }),
                Err(_) => shaky = true,
            }
        }
    }
    if shaky {
        out.unreadable.push(dir.to_owned());
    }
    out
}

fn size(meta: &fs::Metadata) -> i64 {
    i64::try_from(meta.len()).unwrap_or(i64::MAX)
}

/// When a file was last written, in Unix seconds, as the library keeps it.
pub fn mtime(meta: &fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
}

/// A save as the library last read it, for working out what needs reading again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownFile {
    pub id: i64,
    pub path: String,
    pub size: i64,
    pub mtime: i64,
    pub missing: bool,
    /// Read again whatever its date: its key or notes were never worked out, or it was missing.
    pub reread: bool,
}

/// What a scan has to do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// New and changed files to read, in path order.
    pub read: Vec<FoundFile>,
    /// How many of those the library has never seen.
    pub new: usize,
    /// Saves whose file has gone (by `id`), not yet marked missing: a new file can be one of them moved.
    pub gone: Vec<i64>,
    /// Saves the scan leaves out now (an autosave, or in a folder it skips): dropped rather than marked missing.
    pub dropped: Vec<i64>,
}

/// Compares what was found with what the library knows. `reread` says whether a DAW's reader has got better since the
/// library last read its projects; `full` reads everything.
pub fn plan(found: &Found, known: &[KnownFile], roots: &[String], reread: &dyn Fn(&str) -> bool, full: bool) -> Plan {
    let by_path: HashMap<String, &KnownFile> = known.iter().map(|k| (case::key(&k.path), k)).collect();
    let mut out = Plan::default();
    let mut present = HashSet::with_capacity(found.files.len());
    for f in &found.files {
        let key = case::key(&f.path);
        let seen = by_path.get(&key);
        let changed = match seen {
            None => true,
            Some(k) => k.reread || k.mtime != f.mtime || k.size != f.size,
        };
        if full || changed || daw_of(&f.path).is_some_and(reread) {
            out.read.push(f.clone());
            out.new += usize::from(seen.is_none());
        }
        present.insert(key);
    }
    let roots_keys: Vec<String> = roots.iter().map(|r| path_key(r)).collect();
    let unreadable: Vec<String> = found.unreadable.iter().map(|d| path_key(d)).collect();
    for (key, k) in by_path.iter().filter(|(_, k)| !k.missing) {
        if present.contains(key) {
            continue;
        }
        let at = path_key(&k.path);
        if !roots_keys.iter().any(|r| key_inside(&at, r)) || unreadable.iter().any(|d| key_inside(&at, d)) {
            continue;
        }
        if is_ignored(&k.path, roots) {
            out.dropped.push(k.id);
        } else {
            out.gone.push(k.id);
        }
    }
    out.gone.sort_unstable();
    out.dropped.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_files_and_folders_left_out() {
        assert_eq!(daw_of(r"C:\m\Song.FLP"), Some("FL Studio"));
        assert_eq!(daw_of("/m/a.bwproject"), Some("Bitwig Studio"));
        assert_eq!(daw_of("/m/a.song"), Some("Studio One"));
        assert_eq!(daw_of("/m/a.wav"), None);
        assert!(is_project_file(r"C:\m\Night Drive_14.flp"));
        assert!(!is_project_file(r"C:\m\._Night Drive_14.flp"));
        assert!(!is_project_file(r"C:\m\Night Drive (autosave).flp"));
        assert!(!is_project_file(r"C:\m\Night Drive (Overwritten at 2213h05).flp"));
        assert!(skip_dir(".git", "m"));
        assert!(skip_dir("Backup", "Night Drive"));
        assert!(skip_dir("auto-backups", "x"));
        assert!(skip_dir("cool stuff", "Projects"));
        assert!(skip_dir("Patches", "data"));
        assert!(!skip_dir("Patches", "Night Drive"));
        assert!(!skip_dir("Night Drive", "Projects"));
    }

    #[test]
    fn inside_a_folder_not_just_starting_with_its_name() {
        assert!(is_inside(r"K:\Music\a.flp", r"K:\Music"));
        assert!(is_inside(r"k:/music/a.flp", r"K:\Music\"));
        assert!(is_inside(r"K:\Music", r"K:\Music"));
        assert!(!is_inside(r"K:\Music Old\a.flp", r"K:\Music"));
        let roots = vec![r"K:\Music".to_owned()];
        assert!(is_ignored(r"K:\Music\Night Drive\Backup\Night Drive.flp", &roots));
        assert!(is_ignored(r"K:\Music\FL\Data\Patches\demo.flp", &roots));
        assert!(!is_ignored(r"K:\Music\Night Drive\Night Drive.flp", &roots));
        // Only folders below the root count: a root that is itself called "Backup" still scans.
        assert!(!is_ignored(r"K:\Backup\a.flp", &[r"K:\Backup".to_owned()]));
        // A root inside another is the one that counts, and a root spelt in another case or slash still lines up.
        let nested = vec![r"K:\Music".to_owned(), r"K:\Music\Old\Backup".to_owned()];
        assert!(!is_ignored(r"K:\Music\Old\Backup\a.flp", &nested));
        assert!(is_ignored(r"k:/music/Song/BACKUP/a.flp", &roots));
        assert!(is_inside("/m/a.flp", "/"));
    }

    #[test]
    fn gone_paths_that_were_folders() {
        assert!(!looks_like_folder(r"C:\s\kick.wav"));
        assert!(!looks_like_folder(r"C:\s\kick.asd"));
        assert!(!looks_like_folder("/s/a.mp3"));
        assert!(looks_like_folder(r"C:\s\Drums"));
        assert!(looks_like_folder(r"C:\s\Vol.2"));
        assert!(looks_like_folder(r"C:\s\Drums. Dry"));
        assert!(looks_like_folder(r"C:\s\Loops.together"));
    }

    fn found(path: &str, size: i64, mtime: i64) -> FoundFile {
        FoundFile { path: path.into(), size, mtime }
    }

    fn known(id: i64, path: &str, size: i64, mtime: i64) -> KnownFile {
        KnownFile { id, path: path.into(), size, mtime, missing: false, reread: false }
    }

    #[test]
    fn plans_only_what_changed() {
        let roots = vec![r"C:\m".to_owned(), r"D:\gone drive".to_owned()];
        let found = Found {
            files: vec![
                found(r"C:\m\same.flp", 10, 100),
                found(r"C:\m\changed.flp", 11, 100),
                found(r"C:\m\new.flp", 5, 5),
                found(r"C:\m\keyless.flp", 1, 1),
                found(r"C:\m\a.bwproject", 1, 1),
            ],
            unreadable: vec![r"C:\m\Busy".to_owned()],
        };
        let mut keyless = known(4, r"C:\M\KEYLESS.flp", 1, 1);
        keyless.reread = true;
        let mut already = known(9, r"C:\m\lost.flp", 1, 1);
        already.missing = true;
        let library = vec![
            known(1, r"C:\m\same.flp", 10, 100),
            known(2, r"C:\m\changed.flp", 10, 100),
            keyless,
            known(5, r"C:\m\a.bwproject", 1, 1),
            known(6, r"C:\m\deleted.flp", 1, 1),
            known(7, r"C:\m\Busy\held.flp", 1, 1),
            known(8, r"C:\m\Song\Backup\old.flp", 1, 1),
            already,
            known(10, r"E:\not a root\x.flp", 1, 1),
        ];
        let p = plan(&found, &library, &roots, &|daw| daw == "Bitwig Studio", false);
        let read: Vec<&str> = p.read.iter().map(|f| file_name(&f.path)).collect();
        assert_eq!(read, ["changed.flp", "new.flp", "keyless.flp", "a.bwproject"]);
        assert_eq!(p.new, 1);
        assert_eq!(p.gone, [6]);
        assert_eq!(p.dropped, [8]);
        assert_eq!(plan(&found, &library, &roots, &|_| false, true).read.len(), 5);
    }
}
