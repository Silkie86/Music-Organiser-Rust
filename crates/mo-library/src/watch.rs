//! Watching folders (and everything under them) for files appearing, changing, moving or going, as the C# app's
//! `FolderWatch` does: what changed is reported together once the folders have been quiet for a moment, because a DAW
//! writes a project in several steps and a sample pack unzips thousands of files. This is how the app keeps up with
//! your projects while it's open, instead of walking every folder again.
//!
//! When too much changes at once for the system to name every file, or a whole folder moves, the watched folder is
//! reported as overflowed and the caller walks it instead.
//!
//! The quiet wait is written here rather than taken from `notify-debouncer-full` (spec section 12 names it): that one
//! reports each file once *it* has been quiet, while the app wants one report once the whole folder is, with folders
//! to walk again when files can't be named.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use notify::event::{ModifyKind, RemoveKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::scan::{is_ignored, key_inside, looks_like_folder, path_key, skip_dir};

/// What changed while the folders were busy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Files that appeared, changed, moved or went, sorted ignoring case.
    pub files: Vec<String>,
    /// Watched folders where too much changed to name each file, or a folder moved or went: walk these again.
    pub overflowed: Vec<String>,
}

/// The most project files a folder moved in is listed by; past that the watched folder is walked instead.
const MOVED_IN_LIMIT: usize = 5000;

/// The most entries a folder moved in is looked through for project files (a sample pack holds tens of thousands);
/// past that the watched folder is walked instead, by the scan's own threads.
const MOVED_IN_LOOK: usize = 20_000;

/// How long changes wait at most while the folders stay busy (a long recording, a big unzip): a save made meanwhile
/// is reported after this, not only once everything stops.
const LONGEST_WAIT: Duration = Duration::from_secs(10);

/// Watches some folders until it's dropped.
pub struct FolderWatch {
    /// Dropping the watcher ends its events, and with them the thread that gathers them.
    _watcher: RecommendedWatcher,
    count: usize,
}

impl FolderWatch {
    /// Watches `folders` (ones that aren't there, or can't be watched, are left out: the next scan finds their changes)
    /// for files `wanted` says matter, leaving out what the scan leaves out, and calls `report` on a low-priority thread
    /// of its own once nothing has changed for `quiet`.
    pub fn start(
        folders: &[String],
        wanted: fn(&str) -> bool,
        quiet: Duration,
        report: impl FnMut(Changes) + Send + 'static,
    ) -> notify::Result<FolderWatch> {
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut roots: Vec<String> = Vec::new();
        for f in folders.iter().filter(|f| Path::new(f.as_str()).is_dir()) {
            // The same folder written twice, or one inside another, is watched once, as part of the outer one.
            let key = path_key(f);
            if !folders.iter().any(|o| o != f && key_inside(&key, &path_key(o)) && path_key(o) != key)
                && !roots.iter().any(|r| path_key(r) == key)
            {
                roots.push(f.clone());
            }
        }
        // One watcher for every folder; each event is told apart by the folder it's inside.
        let mut watcher = notify::recommended_watcher(move |e: notify::Result<Event>| {
            let _ = tx.send(e);
        })?;
        let mut watched = Vec::new();
        for root in &roots {
            if watcher.watch(Path::new(root), RecursiveMode::Recursive).is_ok() {
                watched.push(root.clone());
            }
        }
        let count = watched.len();
        let gather = Gather::new(watched, wanted);
        thread::Builder::new()
            .name("Folder watch".into())
            .spawn(move || {
                // Rule 21: what it does (looking at a folder moved in) never gets in the way of the window or a DAW.
                let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
                gather.run(rx, quiet, report);
            })
            .map_err(|e| notify::Error::generic(&e.to_string()))?;
        Ok(FolderWatch { _watcher: watcher, count })
    }

    /// How many folders are being watched.
    pub fn count(&self) -> usize {
        self.count
    }
}

#[derive(Default)]
struct Pending {
    /// By each path's [`path_key`], so one file named in two cases or with other slashes is one change.
    files: BTreeMap<String, String>,
    overflowed: BTreeSet<String>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.files.is_empty() && self.overflowed.is_empty()
    }

    fn len(&self) -> usize {
        self.files.len() + self.overflowed.len()
    }

    fn take(&mut self) -> Changes {
        let p = std::mem::take(self);
        Changes { files: p.files.into_values().collect(), overflowed: p.overflowed.into_iter().collect() }
    }
}

struct Gather {
    roots: Vec<String>,
    /// Each root as a [`path_key`], and where it really is (a Mac names `/var/…` as `/private/var/…`).
    keys: Vec<(String, Option<String>)>,
    wanted: fn(&str) -> bool,
    changes: Pending,
}

impl Gather {
    fn new(roots: Vec<String>, wanted: fn(&str) -> bool) -> Gather {
        let keys = roots
            .iter()
            .map(|r| (path_key(r), fs::canonicalize(r).ok().and_then(|c| c.to_str().map(path_key))))
            .collect();
        Gather { roots, keys, wanted, changes: Pending::default() }
    }

    fn run(mut self, rx: mpsc::Receiver<notify::Result<Event>>, quiet: Duration, mut report: impl FnMut(Changes)) {
        // When the last change that mattered came, and when the first still waiting did.
        let (mut last, mut first) = (Instant::now(), Instant::now());
        loop {
            let wait = if self.changes.is_empty() {
                Duration::from_secs(3600)
            } else {
                quiet.saturating_sub(last.elapsed()).min(LONGEST_WAIT.saturating_sub(first.elapsed()))
            };
            match rx.recv_timeout(wait) {
                Ok(event) => {
                    let before = self.changes.len();
                    self.take_in(event);
                    // Only a change that matters starts the quiet wait again: a folder busy with other files (a
                    // recording, an unzip) doesn't hold a save back.
                    if self.changes.len() != before {
                        if before == 0 {
                            first = Instant::now();
                        }
                        last = Instant::now();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if !self.changes.is_empty() && (last.elapsed() >= quiet || first.elapsed() >= LONGEST_WAIT) {
                report(self.changes.take());
            }
        }
    }

    /// The watched folder `path` is in: the innermost, should one be inside another.
    fn root_of(&self, path: &str) -> Option<String> {
        let key = path_key(path);
        self.roots
            .iter()
            .zip(&self.keys)
            .filter(|(_, (k, real))| key_inside(&key, k) || real.as_ref().is_some_and(|r| key_inside(&key, r)))
            .max_by_key(|(_, (k, _))| k.split('/').count())
            .map(|(r, _)| r.clone())
    }

    fn file(&mut self, path: &str) {
        if (self.wanted)(path) && !is_ignored(path, &self.roots) {
            self.changes.files.entry(path_key(path)).or_insert_with(|| path.to_owned());
        }
    }

    fn overflow(&mut self, path: Option<&str>) {
        match path.and_then(|p| self.root_of(p)) {
            Some(root) => {
                self.changes.overflowed.insert(root);
            }
            // Not told where: every watched folder is walked again.
            None => self.changes.overflowed.extend(self.roots.iter().cloned()),
        }
    }

    fn take_in(&mut self, event: notify::Result<Event>) {
        let event = match event {
            Ok(e) => e,
            Err(e) => {
                // The system lost track (its list of changes ran over): walk the folders it names, or all of them.
                let paths: Vec<String> = e.paths.iter().filter_map(|p| p.to_str().map(str::to_owned)).collect();
                if paths.is_empty() {
                    self.overflow(None);
                }
                paths.iter().for_each(|p| self.overflow(Some(p)));
                return;
            }
        };
        let paths: Vec<String> = event.paths.iter().filter_map(|p| p.to_str().map(str::to_owned)).collect();
        if event.need_rescan() {
            if paths.is_empty() {
                self.overflow(None);
            }
            paths.iter().for_each(|p| self.overflow(Some(p)));
            return;
        }
        match event.kind {
            EventKind::Create(_) => paths.iter().for_each(|p| self.appeared(p)),
            EventKind::Remove(RemoveKind::File) => paths.iter().for_each(|p| self.file(p)),
            EventKind::Remove(RemoveKind::Folder) => paths.iter().for_each(|p| self.overflow(Some(p))),
            EventKind::Remove(_) => paths.iter().for_each(|p| self.gone(p)),
            EventKind::Modify(ModifyKind::Name(mode)) => match (mode, paths.as_slice()) {
                (RenameMode::Both, [from, to]) => {
                    if Path::new(to).is_dir() {
                        self.overflow(Some(to));
                    } else {
                        self.file(from);
                        self.file(to);
                    }
                }
                (RenameMode::From, _) => paths.iter().for_each(|p| self.gone(p)),
                (RenameMode::To, _) => paths.iter().for_each(|p| self.appeared(p)),
                // The system couldn't say which end it is: a path that's there now arrived, one that isn't went.
                _ => paths.iter().for_each(|p| if Path::new(p).exists() { self.appeared(p) } else { self.gone(p) }),
            },
            EventKind::Modify(_) | EventKind::Any | EventKind::Other => {
                // A folder's own date changes as files come and go in it; its files report themselves.
                let wanted = self.wanted;
                paths.iter().filter(|p| wanted(p) && !Path::new(p.as_str()).is_dir()).for_each(|p| self.file(p));
            }
            EventKind::Access(_) => {}
        }
    }

    fn appeared(&mut self, path: &str) {
        if !Path::new(path).is_dir() {
            self.file(path);
            return;
        }
        // A folder moved in from elsewhere arrives whole, as one event: its files aren't named one by one, so list
        // them (a folder made empty reports its files itself as they're added, and lists as nothing here).
        let mut look = Look { wanted: self.wanted, files: Vec::new(), seen: 0 };
        if look.list(Path::new(path), 0).is_err() || look.files.len() > MOVED_IN_LIMIT || look.seen > MOVED_IN_LOOK {
            self.overflow(Some(path));
        } else {
            look.files.iter().for_each(|f| self.file(f));
        }
    }

    /// A path that went without the system saying whether it was a file or a folder.
    fn gone(&mut self, path: &str) {
        if (self.wanted)(path) {
            self.file(path);
        } else if looks_like_folder(path) && !mo_formats::paths::file_name(path).starts_with('.') {
            // Most likely a folder: its files aren't named one by one, and it can't be told from a file any more
            // except by its name. A hidden file (".DS_Store") is never taken for one.
            self.overflow(Some(path));
        }
    }
}

/// Looks through a folder moved in for project files.
struct Look {
    wanted: fn(&str) -> bool,
    files: Vec<String>,
    /// Entries looked at so far.
    seen: usize,
}

impl Look {
    /// How deep it goes, as the scan: it also stops a folder link back up its own tree.
    const DEEPEST: usize = 64;

    fn list(&mut self, dir: &Path, depth: usize) -> std::io::Result<()> {
        let parent = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        for entry in fs::read_dir(dir)? {
            if self.files.len() > MOVED_IN_LIMIT || self.seen > MOVED_IN_LOOK {
                return Ok(());
            }
            self.seen += 1;
            let entry = entry?;
            let path = entry.path();
            let Some(name) = path.to_str().map(str::to_owned) else { continue };
            // A link is followed to what it points at, as the scan does.
            let is_dir = match entry.file_type()? {
                k if k.is_symlink() => fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false),
                k => k.is_dir(),
            };
            if is_dir {
                let folder = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                if depth < Self::DEEPEST && !skip_dir(folder, &parent) {
                    self.list(&path, depth + 1)?;
                }
            } else if (self.wanted)(&name) {
                self.files.push(name);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::is_project_file;
    use notify::event::CreateKind;
    use std::path::PathBuf;

    fn gather(roots: &[&str]) -> Gather {
        Gather::new(roots.iter().map(|r| (*r).to_owned()).collect(), is_project_file)
    }

    fn event(kind: EventKind, paths: &[&str]) -> notify::Result<Event> {
        let mut e = Event::new(kind);
        for p in paths {
            e = e.add_path(PathBuf::from(p));
        }
        Ok(e)
    }

    #[test]
    fn gathers_wanted_files_and_overflows() {
        let mut g = gather(&["/m", "/m/inner"]);
        g.take_in(event(EventKind::Create(CreateKind::File), &["/m/Song_2.flp", "/m/kick.wav"]));
        g.take_in(event(EventKind::Modify(ModifyKind::Any), &["/m/SONG_2.flp"]));
        g.take_in(event(EventKind::Modify(ModifyKind::Name(RenameMode::Both)), &["/m/a.flp", "/m/b.flp"]));
        g.take_in(event(EventKind::Remove(RemoveKind::Any), &["/m/inner/Old songs"]));
        g.take_in(event(EventKind::Remove(RemoveKind::Any), &["/m/kick.wav"]));
        // A backup copy is left out, as the scan leaves it out; a Mac's ".DS_Store" going isn't a folder going.
        g.take_in(event(EventKind::Create(CreateKind::File), &["/m/Song/Backup/Song_2.flp"]));
        g.take_in(event(EventKind::Remove(RemoveKind::Any), &["/m/.DS_Store"]));
        let c = g.changes.take();
        assert_eq!(c.files, ["/m/a.flp", "/m/b.flp", "/m/Song_2.flp"]);
        assert_eq!(c.overflowed, ["/m/inner"]);
        assert!(g.changes.is_empty());
        g.take_in(Err(notify::Error::generic("too many changes")));
        assert_eq!(g.changes.take().overflowed, ["/m", "/m/inner"]);
        // The system said it was a folder, whatever its name looks like.
        g.take_in(event(EventKind::Remove(RemoveKind::Folder), &["/m/Mixes.old"]));
        g.take_in(Err(notify::Error::generic("lost track").add_path(PathBuf::from("/m/inner/x"))));
        assert_eq!(g.changes.take().overflowed, ["/m", "/m/inner"]);
        g.take_in(event(EventKind::Access(notify::event::AccessKind::Any), &["/m/c.flp"]));
        assert!(g.changes.is_empty());
    }
}
