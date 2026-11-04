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
//!    folder it's in must have the same name too, since those folders hold thousands of sounds;
//! 7. in the Sounds library (the folders the Sounds page scans, as `sounds.db` lists them), when the locator is given
//!    it: a file with the same name in a folder with the same name, or else the same name anywhere when every sound of
//!    that name is the same size (copies of one sound, so whichever is found is the right one). Solomon, 2026-10-31:
//!    old saves point at drives long gone ("D:\harrys samples\…"), and their samples may be in the library now.
//!    No DAW saves a sample's file size (FL Studio keeps only the path; Studio One its length in frames), so the size
//!    checked is the library's own. The library is read the first time a sample gets this far, so a project whose
//!    samples are all in its own or its DAW's folders never waits for it.
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

/// The most entries (files and folders of every kind) looked at in one folder tree: a project saved in a big folder
/// with few sounds in it (C:\Users\Solomon) isn't walked to the end either.
pub const MOST_ENTRIES: usize = 1_000_000;

/// The project's own folder and the folders beside it that DAWs collect samples into.
const BESIDE: [&str; 6] = ["", "samples", "recordings", "Media", "Audio", "audio"];

/// Folders inside a DAW's folders that hold projects, autosaves or history rather than sounds.
const NOT_CONTENT: [&str; 8] =
    ["Projects", "Songs", "Shows", "auto-backups", "Backup", "History", "$RECYCLE.BIN", "System Volume Information"];

/// Folders below a tree's top that hold programs' own files, never sounds, so a sketch saved in the profile folder
/// doesn't walk all of AppData. A DAW folder that is itself in AppData is still read: only the folders below the one
/// asked for are left out.
const NOT_SOUNDS: [&str; 2] = ["AppData", "node_modules"];

/// The most folder trees a locator keeps at once.
const MOST_TREES: usize = 4096;

/// Where the DAW's own content folders are for a project, by its path.
type ContentFolders = dyn Fn(&str) -> Vec<String> + Send + Sync;

/// A folder tree's index, read the first time it's wanted.
type Tree = Arc<OnceLock<FolderIndex>>;

/// What gives the Sounds library when it's first wanted.
type ReadLibrary = dyn Fn() -> Arc<SoundLibrary> + Send + Sync;

/// Finds samples, reading each folder tree once. Shared between the threads of one scan.
pub struct SampleLocator {
    content: Box<ContentFolders>,
    trees: Mutex<HashMap<(String, usize), Tree>>,
    library: Option<LibrarySlot>,
}

/// The Sounds library, read the first time a sample isn't found anywhere else (rule 7 above).
struct LibrarySlot {
    read: Box<ReadLibrary>,
    library: OnceLock<Arc<SoundLibrary>>,
}

impl LibrarySlot {
    /// The library, read now if it hasn't been yet: another thread wanting it meanwhile waits for this read.
    fn get(&self) -> &SoundLibrary {
        self.library.get_or_init(|| (self.read)())
    }
}

impl Default for SampleLocator {
    fn default() -> SampleLocator {
        SampleLocator::new(Box::new(daw_content_folders))
    }
}

impl SampleLocator {
    /// A locator that looks in `content` for a DAW's own folders (tests give their own).
    pub fn new(content: Box<ContentFolders>) -> SampleLocator {
        SampleLocator { content, trees: Mutex::new(HashMap::new()), library: None }
    }

    /// The same locator, looking in the Sounds library last (rule 7 above).
    pub fn with_library(self, library: Arc<SoundLibrary>) -> SampleLocator {
        self.with_library_from(move || library.clone())
    }

    /// The same locator, looking last in the Sounds library that `read` gives (rule 7 above). It's asked for the
    /// library the first time a sample isn't in the project's or the DAW's folders, and only then.
    pub fn with_library_from(mut self, read: impl Fn() -> Arc<SoundLibrary> + Send + Sync + 'static) -> SampleLocator {
        self.library = Some(LibrarySlot { read: Box::new(read), library: OnceLock::new() });
        self
    }

    /// Whether this sample is on this computer, by the rules above. FL Studio's own content
    /// ("%FLStudioFactoryData%…") always is; as in the C# app, so is any path that starts with a "%" setting.
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
        // A project kept at the top of a drive or a network share: only the files beside it, not the whole drive.
        let dir = project_dir.to_str()?;
        let (root, folders) = split(dir);
        let at_top = !root.is_empty() && folders.len() <= if root == r"\\" { 2 } else { 0 };
        if let Some(found) = self.find_in(dir, if at_top { 0 } else { usize::MAX }, name, None) {
            return Some(found);
        }
        let folder = parts.len().checked_sub(2).and_then(|i| parts.get(i)).copied();
        let content = (self.content)(project);
        let in_content = match folder {
            Some(_) => content.iter().find_map(|c| self.find_in(c, usize::MAX, name, folder)),
            // A sample saved as a bare file name has no folder to match: those folders hold thousands of sounds, so
            // the name alone counts only when one file in them has it.
            None => {
                // Each file once, across every folder: they can hold one another (Bitwig Studio and its Library).
                let mut seen = HashSet::new();
                let mut all = content
                    .iter()
                    .flat_map(|c| self.named_in(c, name))
                    .filter(|f| seen.insert(case::key(f)) && Path::new(f.as_str()).is_file());
                match (all.next(), all.next()) {
                    (Some(only), None) => Some(only),
                    _ => None,
                }
            }
        };
        in_content.or_else(|| self.library.as_ref()?.get().find(name, folder))
    }

    /// A file called `name` in the tree under `root` (in a folder called `folder`, when given), reading the tree the
    /// first time it's asked for.
    fn find_in(&self, root: &str, depth: usize, name: &str, folder: Option<&str>) -> Option<String> {
        self.tree(root, depth).get()?.find(name, folder)
    }

    /// Every file called `name` the tree under `root` had when it was read.
    fn named_in(&self, root: &str, name: &str) -> Vec<String> {
        let tree = self.tree(root, usize::MAX);
        tree.get().and_then(|index| index.by_name.get(&case::key(name)).cloned()).unwrap_or_default()
    }

    fn tree(&self, folder: &str, depth: usize) -> Tree {
        // By depth too: the top of a drive read for the files beside a project isn't the whole drive.
        let key = (case::key(folder.trim_end_matches(['\\', '/'])), depth);
        let slot = {
            let Ok(mut trees) = self.trees.lock() else { return Arc::new(OnceLock::new()) };
            let slot = trees.entry(key.clone()).or_default().clone();
            // Too many kept: let the rest go, as the C# app does, keeping the one being read.
            if trees.len() > MOST_TREES {
                trees.retain(|k, _| *k == key);
            }
            slot
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
        let mut entries_seen = 0;
        let mut todo: Vec<(PathBuf, usize)> = vec![(PathBuf::from(root), 0)];
        // Where linked folders really go, so a link back up the tree, or two links to one folder, is walked once.
        let real_root = fs::canonicalize(root).ok();
        let mut linked: HashSet<PathBuf> = HashSet::new();
        while let Some((dir, level)) = todo.pop() {
            if files >= MOST_FILES || entries_seen >= MOST_ENTRIES {
                break;
            }
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            let mut dirs = Vec::new();
            for entry in entries.flatten() {
                entries_seen += 1;
                if entries_seen > MOST_ENTRIES {
                    break;
                }
                let path = entry.path();
                let Some(p) = path.to_str() else { continue };
                let Ok(kind) = entry.file_type() else { continue };
                // A link is looked at as what it points to, as Windows' own listing does.
                let is_dir = if kind.is_symlink() {
                    let Ok(real) = fs::canonicalize(&path) else { continue };
                    if real.is_dir() {
                        let inside = real_root.as_ref().is_some_and(|r| real.starts_with(r));
                        if inside || !linked.insert(real) {
                            continue;
                        }
                        true
                    } else {
                        false
                    }
                } else {
                    kind.is_dir()
                };
                if is_dir {
                    let name = file_name(p);
                    if level < depth
                        && !name.starts_with('.')
                        && !NOT_CONTENT.iter().chain(&NOT_SOUNDS).any(|n| case::eq_ignore_case(n, name))
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

/// Every sound's id and path, which SQLite reads from the library's index of paths alone (a "covering index"): on a
/// test library the size of Solomon's (453,885 sounds, 5.1 GB) that's 46 MB. Each sound's analysis and fingerprints
/// sit beside its path in the table, so reading the paths from there went through 1.9 GB of it.
const PATHS_SQL: &str = "SELECT id, path FROM sounds";

/// A sound's size, found by its path in that same index.
const SIZE_SQL: &str = "SELECT COALESCE(size, 0) FROM sounds WHERE path = ?1";

/// The sounds the Sounds page knows, by file name: read once from `sounds.db`, never written. Only their paths are
/// read; a sound's size is looked up when the rule above needs it.
#[derive(Default)]
pub struct SoundLibrary {
    /// Each name's sounds, in the order they joined the library.
    by_name: HashMap<String, Vec<String>>,
    sizes: Sizes,
    /// Whether every sound was read: not when the database couldn't be opened or a row couldn't be read.
    complete: bool,
    /// Whether the sounds of a name are all copies of one sound, worked out once for each name.
    copies: Mutex<HashMap<String, bool>>,
}

/// Where a library's sizes come from.
#[derive(Default)]
enum Sizes {
    /// None known, so no name is taken for copies of one sound.
    #[default]
    Unknown,
    /// Given with the files, by path.
    Given(HashMap<String, i64>),
    /// Looked up in the `sounds.db` the paths came from.
    Database(PathBuf),
}

impl SoundLibrary {
    /// The sounds in the Sounds page's database, opened read-only. An empty library when it can't be read (no
    /// database yet, or one in use in a way that stops reading): samples are then looked for as before.
    pub fn read(sounds_db: &Path) -> SoundLibrary {
        let Some(conn) = open(sounds_db) else { return SoundLibrary::default() };
        let Ok(mut st) = conn.prepare(PATHS_SQL) else { return SoundLibrary::default() };
        let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))) else {
            return SoundLibrary::default();
        };
        // A row that can't be read ends the read (SQLite gives nothing after it): what came before is kept.
        let mut complete = true;
        let mut files: Vec<(i64, String)> = Vec::new();
        for row in rows {
            match row {
                Ok(file) => files.push(file),
                Err(_) => {
                    complete = false;
                    break;
                }
            }
        }
        // The index gives them by path: put back in the order they joined (as the table gives them), so the same copy
        // of a sound is found first as before.
        files.sort_unstable_by_key(|(id, _)| *id);
        let mut library =
            SoundLibrary { sizes: Sizes::Database(sounds_db.to_owned()), complete, ..SoundLibrary::default() };
        for (_, path) in files {
            library.add(path);
        }
        library
    }

    /// A library of these files and sizes (tests give their own).
    pub fn from_files(files: impl IntoIterator<Item = (String, i64)>) -> SoundLibrary {
        let mut library = SoundLibrary { complete: true, ..SoundLibrary::default() };
        let mut sizes = HashMap::new();
        for (path, size) in files {
            sizes.insert(path.clone(), size);
            library.add(path);
        }
        library.sizes = Sizes::Given(sizes);
        library
    }

    /// Whether every sound was read. A library that wasn't (the database missing, or busy beyond waiting for) is worth
    /// reading again soon, even if the database hasn't changed since.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    fn add(&mut self, path: String) {
        self.by_name.entry(case::key(file_name(&path))).or_default().push(path);
    }

    /// How many sounds it knows.
    pub fn len(&self) -> usize {
        self.by_name.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// A sound called `name` that's still there: in a folder called `folder` first, or else anywhere when every
    /// sound of that name is the same size, so they're copies of one sound and not different sounds sharing a name.
    fn find(&self, name: &str, folder: Option<&str>) -> Option<String> {
        let key = case::key(name);
        let all = self.by_name.get(&key)?;
        let there = |p: &&String| Path::new(p.as_str()).is_file();
        if let Some(want) = folder {
            let parent = |p: &str| split(p).1.iter().rev().nth(1).is_some_and(|f| same(f, want));
            if let Some(p) = all.iter().filter(|p| parent(p)).find(there) {
                return Some(p.clone());
            }
        }
        if !self.copies_of_one(&key, all) {
            return None;
        }
        all.iter().find(there).cloned()
    }

    /// Whether the sounds in `all` (the sounds called `key`) are copies of one sound, by the library's own sizes.
    /// Worked out the first time a name is asked about, then kept as long as the library: no for a name whose sizes
    /// couldn't be looked up too, so the database isn't asked again and again while it can't answer. The library is
    /// read again once the database changes.
    fn copies_of_one(&self, key: &str, all: &[String]) -> bool {
        if let Some(known) = self.copies.lock().ok().and_then(|c| c.get(key).copied()) {
            return known;
        }
        let copies = match &self.sizes {
            Sizes::Unknown => false,
            Sizes::Given(sizes) => one_size(all.iter().map(|p| sizes.get(p).copied())),
            Sizes::Database(db) => sizes_in(db, all),
        };
        if let Ok(mut known) = self.copies.lock() {
            known.insert(key.to_owned(), copies);
        }
        copies
    }
}

/// Whether the sounds at `paths` have one size in the library at `db`, as [`one_size`] works it out. Not when it can't
/// be asked, nor when a sound has left the library since its paths were read (no size to compare with).
fn sizes_in(db: &Path, paths: &[String]) -> bool {
    let Some(conn) = open(db) else { return false };
    let Ok(mut st) = conn.prepare(SIZE_SQL) else { return false };
    one_size(paths.iter().map(|p| st.query_row([p], |r| r.get::<_, i64>(0)).ok()))
}

/// Whether all of these are one size, looking no further than the first that differs: not when one differs, isn't
/// known, or is nothing (a sound the library has no size for).
fn one_size(mut sizes: impl Iterator<Item = Option<i64>>) -> bool {
    match sizes.next() {
        Some(Some(first)) if first > 0 => sizes.all(|s| s == Some(first)),
        _ => false,
    }
}

/// The Sounds page's database, opened read-only.
fn open(sounds_db: &Path) -> Option<rusqlite::Connection> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = rusqlite::Connection::open_with_flags(sounds_db, flags).ok()?;
    // The C# app may be saving a scan: wait a little for it rather than fail.
    conn.busy_timeout(std::time::Duration::from_secs(2)).ok()?;
    Some(conn)
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

/// The fixed folders, worked out once for each kind of project: they don't change while the app runs.
fn fixed_content_folders(ext: &str) -> Vec<String> {
    static KNOWN: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Mutex::default);
    if let Some(f) = known.lock().ok().and_then(|k| k.get(ext).cloned()) {
        return f;
    }
    let found = look_for_content_folders(ext);
    if let Ok(mut k) = known.lock() {
        k.insert(ext.to_owned(), found.clone());
    }
    found
}

fn look_for_content_folders(ext: &str) -> Vec<String> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = env(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    // Windows and a Mac keep the DAWs' user folders in Documents; on Linux they sit in the home folder. Windows says
    // where Documents really is: it may have moved (to OneDrive by its Documents backup, or to another drive).
    #[cfg(windows)]
    let moved = known_folders::get_known_folder_path(known_folders::KnownFolder::Documents);
    #[cfg(not(windows))]
    let moved: Option<PathBuf> = None;
    let documents = moved
        .or_else(|| home.as_ref().map(|h| if cfg!(target_os = "linux") { h.clone() } else { h.join("Documents") }));
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
    fn the_library_is_read_from_its_index_of_paths() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // The C# app's sounds table (`SoundLibrary.Schema`).
        conn.execute_batch(
            "CREATE TABLE sounds (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, size INTEGER,
                 mtime INTEGER, error TEXT, analysis TEXT, features BLOB, clap BLOB, name_type TEXT, user_type TEXT,
                 type TEXT, type_source TEXT, type_conf REAL, favourite INTEGER DEFAULT 0, rating INTEGER DEFAULT 0,
                 plays INTEGER DEFAULT 0, last_played INTEGER)",
        )
        .unwrap();
        let plan = |sql: &str, params: &[&str]| -> String {
            let mut st = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let rows = st.query_map(rusqlite::params_from_iter(params), |r| r.get::<_, String>(3)).unwrap();
            rows.map(Result::unwrap).collect::<Vec<_>>().join("; ")
        };
        // From the index alone, never the table's rows (where each path sits beside kilobytes of analysis).
        let paths = plan(PATHS_SQL, &[]);
        assert!(paths.contains("USING COVERING INDEX"), "{paths}");
        // One sound's size: found by its path in the index, then its row.
        let size = plan(SIZE_SQL, &["x"]);
        assert!(size.contains("USING INDEX") && size.contains("path=?"), "{size}");
    }

    #[test]
    fn copies_of_one_sound_have_one_size() {
        assert!(one_size([Some(6), Some(6)].into_iter()));
        assert!(one_size([Some(6)].into_iter()));
        // Two different sounds sharing a name, one the library has no size for, or one gone from it: no guess.
        assert!(!one_size([Some(6), Some(5)].into_iter()));
        assert!(!one_size([Some(0), Some(0)].into_iter()));
        assert!(!one_size([Some(6), None].into_iter()));
        assert!(!one_size([None, Some(6)].into_iter()));
        assert!(!one_size(std::iter::empty()));
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
